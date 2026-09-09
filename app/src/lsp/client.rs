//! LSP transport + client manager — the analogue of Zed's `crates/lsp` and
//! `crates/project/src/lsp_store.rs`.
//!
//! Spawns and communicates with language servers over stdio using JSON-RPC
//! and `lsp-types`. Diagnostics are streamed to the workspace and rendered as
//! real-time squiggly underlines.
//!
//! Which server to run, how to invoke it and what to configure it with all
//! come from [`super::adapter`]; installing Node-based servers is
//! [`super::node`]'s job. This module only owns the protocol.
//!
//! Besides the streaming notifications, the client supports synchronous
//! request/response pairs (`textDocument/completion`, `textDocument/hover`,
//! `textDocument/definition`, `textDocument/codeAction`,
//! `textDocument/formatting`, …) which are exposed to the editor through the
//! provider hooks of gpui-component's `InputState::lsp` — the same surface
//! Zed's editor uses to talk to language servers.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::ops::Range as StdRange;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use gpui::{App, AppContext, Context, Entity, SharedString, Task, Window};
use gpui_component::input::{
    CodeActionProvider, CompletionProvider, DefinitionProvider, HoverProvider, InputState, Rope,
    RopeExt,
};
use lsp_types::{
    ClientCapabilities, CodeAction, CodeActionContext, CodeActionOrCommand, CodeActionParams,
    CodeActionResponse, CodeActionTriggerKind, CompletionContext, CompletionParams,
    CompletionResponse, CompletionTriggerKind, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DocumentFormattingParams,
    FormattingOptions, GotoDefinitionResponse, Hover, InitializeParams, InitializedParams,
    Location, LocationLink, PublishDiagnosticsClientCapabilities, PublishDiagnosticsParams,
    TextDocumentClientCapabilities, TextDocumentContentChangeEvent, TextDocumentIdentifier,
    TextDocumentItem, TextDocumentPositionParams, TextDocumentSyncClientCapabilities, Uri,
    VersionedTextDocumentIdentifier,
};
use serde_json::{json, Value};

/// How long a request may take before we give up (servers that are
/// initializing a large project can be slow on the first request).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Debug)]
pub enum LspEvent {
    Diagnostics {
        path: PathBuf,
        diagnostics: Vec<lsp_types::Diagnostic>,
    },
    /// A transient message for the status bar.
    Status {
        lang: String,
        message: String,
    },
    /// A background install finished; the server can now be started.
    ServerReady {
        server: String,
    },
    /// A server could not be installed or started.
    ServerFailed {
        server: String,
        reason: String,
    },
    /// The `initialize` handshake completed.
    Initialized {
        server: String,
    },
    /// The server process exited unexpectedly (crash, OOM, kill). The
    /// workspace drops the cached client and respawns the server for every
    /// open buffer, so a crashed `tsserver` heals itself instead of leaving
    /// the editor silently feature-less until the file is reopened.
    ServerExited {
        server: String,
    },
}

/// Lifecycle of one language server, surfaced in the status bar.
///
/// Zed shows the same progression in its status bar / "language server logs":
/// checking → downloading/installing → starting → running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerStatus {
    /// npm install in progress (Zed: "Downloading <server>…").
    Installing,
    /// Process spawned, `initialize` handshake in flight.
    Starting,
    /// Handshake finished; diagnostics and completions are live.
    Running,
    /// Not usable, with a user-facing reason.
    Failed(String),
}

/// Owns one `LspClient` per *server*, spawns them on demand and installs
/// Node-based servers in the background.
///
/// Note the key change versus a naive design: clients are keyed by **server
/// name**, not by language. `typescript-language-server` serves `.ts`, `.js`,
/// `.tsx` and `.jsx` from a single process that shares one project graph —
/// which is what lets a rename in `a.ts` show an error in `b.tsx`. Keying by
/// language would spawn four servers that each see a quarter of the project.
/// Zed does the same via `LanguageServerName`.
pub struct LspManager {
    /// server name → client
    clients: HashMap<String, Arc<LspClient>>,
    /// server name → status
    statuses: HashMap<String, ServerStatus>,
    /// Servers whose background install has been kicked off already.
    installing: HashSet<String>,
    root: Option<PathBuf>,
    event_tx: async_channel::Sender<LspEvent>,
    event_rx: async_channel::Receiver<LspEvent>,
    /// server name → (consecutive crash count, last crash timestamp)
    crash_counts: HashMap<String, (usize, Instant)>,
}

impl LspManager {
    pub fn new() -> Self {
        let (event_tx, event_rx) = async_channel::unbounded();
        Self {
            clients: HashMap::new(),
            statuses: HashMap::new(),
            installing: HashSet::new(),
            root: None,
            event_tx,
            event_rx,
            crash_counts: HashMap::new(),
        }
    }

    pub fn event_receiver(&self) -> async_channel::Receiver<LspEvent> {
        self.event_rx.clone()
    }

    /// Set the workspace root. Servers started later are rooted here, which is
    /// what lets them read `tsconfig.json` / `package.json` and resolve
    /// imports across the project.
    pub fn set_root(&mut self, root: Option<PathBuf>) {
        if self.root != root {
            self.root = root;
            // Restart everything: a language server's project graph is bound
            // to its root, so an old client would keep serving the old
            // project. Zed likewise restarts servers when worktrees change.
            self.clients.clear();
            self.statuses.clear();
            self.installing.clear();
            self.crash_counts.clear();
        }
    }

    #[allow(dead_code)]
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Ensure a server is running for `lang`, installing it first if needed.
    ///
    /// Never blocks: if the server has to be installed, this kicks off a
    /// background `npm install`, reports [`ServerStatus::Installing`] and
    /// returns `None`. The install thread emits [`LspEvent::ServerReady`]
    /// when it finishes so the UI can retry and open its documents.
    pub fn ensure_server(
        &mut self,
        lang: &str,
        root_dir: Option<&Path>,
    ) -> Option<Arc<LspClient>> {
        let adapter = super::adapter::adapter_for_language(lang)?;
        let name = adapter.name;

        if let Some(client) = self.clients.get(name) {
            if client.is_alive() {
                return Some(client.clone());
            }
            // The server died (crash, OOM). Drop it and respawn below —
            // Zed's supervisor does the same.
            self.clients.remove(name);
        }

        if let Some(status) = self.statuses.get(name) {
            if matches!(status, ServerStatus::Failed(_)) {
                if let Some((count, last_crash)) = self.crash_counts.get(name) {
                    if *count >= 3 && last_crash.elapsed() < Duration::from_secs(60) {
                        return None;
                    }
                }
            }
        }

        let root = root_dir.map(Path::to_path_buf).or_else(|| self.root.clone());

        match adapter.source {
            super::adapter::Source::Native { binary } => {
                let Some(program) = find_binary_on_path(binary) else {
                    self.statuses.insert(
                        name.to_string(),
                        ServerStatus::Failed(format!("{binary} is not installed or not on PATH")),
                    );
                    return None;
                };
                self.spawn_client(adapter, program, adapter.args.iter().map(|s| s.to_string()).collect(), root)
            }
            super::adapter::Source::Npm { package, entry } => {
                match super::node::resolve_npm_server(name, entry, adapter.args) {
                    super::node::Resolved::Ready { program, args } => {
                        self.spawn_client(adapter, program, args, root)
                    }
                    super::node::Resolved::Unavailable(reason) => {
                        self.statuses
                            .insert(name.to_string(), ServerStatus::Failed(reason));
                        None
                    }
                    super::node::Resolved::NeedsInstall => {
                        self.start_install(name, package, entry, adapter);
                        None
                    }
                }
            }
        }
    }

    /// Spawn a client for a resolved executable and record its status.
    fn spawn_client(
        &mut self,
        adapter: &'static super::adapter::ServerAdapter,
        program: PathBuf,
        args: Vec<String>,
        root: Option<PathBuf>,
    ) -> Option<Arc<LspClient>> {
        let name = adapter.name;
        match LspClient::spawn(adapter, program, args, root.as_deref(), self.event_tx.clone()) {
            Some(client) => {
                let client = Arc::new(client);
                self.clients.insert(name.to_string(), client.clone());
                self.statuses
                    .insert(name.to_string(), ServerStatus::Starting);
                Some(client)
            }
            None => {
                self.statuses.insert(
                    name.to_string(),
                    ServerStatus::Failed(format!("failed to start {name}")),
                );
                None
            }
        }
    }

    /// Kick off a background npm install for a server (once).
    fn start_install(
        &mut self,
        name: &'static str,
        package: &'static str,
        entry: &'static str,
        adapter: &'static super::adapter::ServerAdapter,
    ) {
        if !self.installing.insert(name.to_string()) {
            return;
        }
        self.statuses
            .insert(name.to_string(), ServerStatus::Installing);

        let mut packages = vec![format!("{package}@latest")];
        packages.extend(adapter.extra_npm_packages().iter().map(|p| p.to_string()));

        let tx = self.event_tx.clone();
        std::thread::spawn(move || {
            let _ = tx.try_send(LspEvent::Status {
                lang: name.to_string(),
                message: format!("Installing {package}…"),
            });
            match super::node::install_npm_server(name, &packages, entry) {
                Ok(_) => {
                    let _ = tx.try_send(LspEvent::ServerReady {
                        server: name.to_string(),
                    });
                }
                Err(e) => {
                    let _ = tx.try_send(LspEvent::ServerFailed {
                        server: name.to_string(),
                        reason: e,
                    });
                }
            }
        });
    }

    /// Clear the in-flight install flag so the server can be started.
    pub fn finish_install(&mut self, server: &str) {
        self.installing.remove(server);
    }

    /// Record a terminal failure for a server.
    pub fn set_failed(&mut self, server: &str, reason: String) {
        self.installing.remove(server);
        self.statuses
            .insert(server.to_string(), ServerStatus::Failed(reason));
    }

    /// Mark a server as fully initialized.
    pub fn set_running(&mut self, server: &str) {
        self.crash_counts.remove(server);
        self.statuses
            .insert(server.to_string(), ServerStatus::Running);
    }

    /// The server process exited unexpectedly: forget the client (dropping
    /// the last `Arc` kills the child) and show it as starting again, so the
    /// next `ensure_server` — usually right after, from
    /// `start_server_for_open_buffers` — respawns a fresh process.
    ///
    /// Returns `true` only when a live client was actually removed. Normal
    /// teardown (app quit, root change) drops already-removed clients, and
    /// those must not trigger a respawn.
    pub fn drop_client(&mut self, server: &str) -> bool {
        let removed = self.clients.remove(server).is_some();
        if !removed {
            return false;
        }

        let now = Instant::now();
        let (count, last_crash) = self
            .crash_counts
            .entry(server.to_string())
            .or_insert((0, now));

        if now.duration_since(*last_crash) < Duration::from_secs(10) {
            *count += 1;
        } else {
            *count = 1;
        }
        *last_crash = now;

        if *count >= 3 {
            self.statuses.insert(
                server.to_string(),
                ServerStatus::Failed(format!("{server} exited repeatedly — stopped auto-restarting")),
            );
            eprintln!("[LSP] {server} exited repeatedly — stopping auto-restart.");
            false
        } else {
            self.statuses
                .insert(server.to_string(), ServerStatus::Starting);
            true
        }
    }

    /// The languages a server handles, for reopening documents after install.
    pub fn languages_for_server(&self, server: &str) -> &'static [&'static str] {
        super::adapter::adapter_by_name(server)
            .map(|a| a.languages)
            .unwrap_or(&[])
    }

    /// The running client for `lang`, if any (no spawning, no installing).
    pub fn client_for(&self, lang: &str) -> Option<Arc<LspClient>> {
        let adapter = super::adapter::adapter_for_language(lang)?;
        self.clients
            .get(adapter.name)
            .cloned()
            .filter(|c| c.is_alive())
    }

    /// Status of the server that handles `lang`.
    pub fn status_for_language(&self, lang: &str) -> Option<ServerStatus> {
        let adapter = super::adapter::adapter_for_language(lang)?;
        // A live, initialized client outranks a stale stored status.
        if let Some(client) = self.clients.get(adapter.name) {
            if client.is_alive() {
                return Some(if client.is_ready() {
                    ServerStatus::Running
                } else {
                    ServerStatus::Starting
                });
            }
        }
        self.statuses.get(adapter.name).cloned()
    }

    /// The server name handling `lang`, for status display.
    pub fn server_name_for_language(&self, lang: &str) -> Option<&'static str> {
        super::adapter::adapter_for_language(lang).map(|a| a.name)
    }

    pub fn change_document(&mut self, path: &Path, lang: &str, text: String) {
        if let Some(client) = self.client_for(lang) {
            client.did_change(path, text);
        }
    }

    pub fn close_document(&mut self, path: &Path, lang: &str) {
        if let Some(client) = self.client_for(lang) {
            client.did_close(path);
        }
    }

    /// Notify the server that a document was saved. Servers like ESLint and
    /// gopls only run some checks on save (Zed sends this from its buffer
    /// store on every save).
    pub fn save_document(&mut self, path: &Path, lang: &str, text: &str) {
        if let Some(client) = self.client_for(lang) {
            client.did_save(path, text);
        }
    }

    /// True when a live client exists for `lang`.
    ///
    /// Lets the UI skip whole-buffer reads and coalescing work on every
    /// keystroke when no server exists for the language.
    pub fn has_client(&self, lang: &str) -> bool {
        self.client_for(lang).is_some()
    }
}

pub struct LspClient {
    /// The adapter that configures this server (Zed's `LspAdapter`).
    pub adapter: &'static super::adapter::ServerAdapter,
    /// Workspace root this server was started in.
    #[allow(dead_code)]
    pub root: Option<PathBuf>,
    /// Outbound framed messages, drained by a dedicated writer thread so a
    /// slow language server (full stdin pipe) can never stall the UI thread.
    /// The channel is unbounded, so `send` never blocks.
    out: mpsc::Sender<Vec<u8>>,
    versions: Arc<Mutex<HashMap<PathBuf, i32>>>,
    is_alive: Arc<Mutex<bool>>,
    is_initialized: Arc<Mutex<bool>>,
    pending_opens: Arc<Mutex<Vec<(PathBuf, String, String)>>>,
    /// Documents whose text changed but haven't been synced yet: latest text
    /// per path. The writer thread flushes these as debounced full-document
    /// `didChange` messages, coalescing a fast typist's keystrokes into a
    /// single sync instead of one whole-document message per keystroke.
    pending_changes: Arc<Mutex<HashMap<PathBuf, String>>>,
    /// The most recent text we told the server for each open document. Used
    /// by providers to answer requests that need positions (code actions).
    last_texts: Arc<Mutex<HashMap<PathBuf, String>>>,
    /// What the server currently holds for each document — the base for
    /// incremental `didChange` diffs. Deliberately separate from
    /// `last_texts`: that mirrors the *editor's* text (updated on every
    /// keystroke, before the debounced flush), and diffing the editor text
    /// against itself would silently drop the edit entirely.
    synced_texts: Arc<Mutex<HashMap<PathBuf, String>>>,
    /// Latest diagnostics per document (from publishDiagnostics), used to
    /// build the `CodeActionContext` of code-action requests.
    last_diagnostics: Arc<Mutex<HashMap<PathBuf, Vec<lsp_types::Diagnostic>>>>,
    /// Capabilities advertised by the server in its `initialize` response.
    server_capabilities: Arc<Mutex<Option<lsp_types::ServerCapabilities>>>,
    /// Monotonic request id allocator. Starts at 100 so the reserved
    /// `initialize` id (1) never collides with a routed response.
    next_id: AtomicI64,
    /// In-flight requests: id → channel that the reader thread delivers the
    /// response on.
    pending: Arc<Mutex<HashMap<i64, mpsc::Sender<Value>>>>,
    child: Arc<Mutex<Option<Child>>>,
}

impl LspClient {
    /// Spawn an already-resolved server executable.
    ///
    /// `program` + `args` come from the adapter layer: for Node servers that
    /// is our managed `node` plus the server's JS entry point, exactly like
    /// Zed's `LanguageServerBinary`. No shell, no `.cmd` shim, no PATH
    /// guessing at this level.
    pub fn spawn(
        adapter: &'static super::adapter::ServerAdapter,
        program: PathBuf,
        args: Vec<String>,
        root_dir: Option<&Path>,
        event_tx: async_channel::Sender<LspEvent>,
    ) -> Option<Self> {
        let mut cmd = Command::new(&program);
        cmd.args(&args);

        if let Some(root) = root_dir {
            cmd.current_dir(root);
        }

        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        // Keep stderr: language servers report fatal misconfiguration there,
        // and swallowing it is why "no diagnostics" bugs are so hard to
        // diagnose. A reader thread below logs it.
        cmd.stderr(Stdio::piped());

        // Don't pop up a console window for each server on Windows.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                eprintln!("[LSP] failed to spawn '{}': {e}", adapter.name);
                return None;
            }
        };

        // Drain stderr so a chatty server can never fill the pipe and wedge.
        if let Some(stderr) = child.stderr.take() {
            let name = adapter.name;
            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    eprintln!("[LSP: {name}] {line}");
                }
            });
        }

        let stdin = child.stdin.take()?;
        let stdout = child.stdout.take()?;

        let stdin_arc = Arc::new(Mutex::new(Some(stdin)));
        let is_alive = Arc::new(Mutex::new(true));
        let is_initialized = Arc::new(Mutex::new(false));
        let pending_opens = Arc::new(Mutex::new(Vec::new()));
        let versions = Arc::new(Mutex::new(HashMap::new()));
        let pending_changes: Arc<Mutex<HashMap<PathBuf, String>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let last_texts: Arc<Mutex<HashMap<PathBuf, String>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let synced_texts: Arc<Mutex<HashMap<PathBuf, String>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let last_diagnostics: Arc<Mutex<HashMap<PathBuf, Vec<lsp_types::Diagnostic>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let server_capabilities: Arc<Mutex<Option<lsp_types::ServerCapabilities>>> =
            Arc::new(Mutex::new(None));
        let pending: Arc<Mutex<HashMap<i64, mpsc::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let child_arc = Arc::new(Mutex::new(Some(child)));

        // Framed-message channel drained by a dedicated writer thread. All
        // sends are non-blocking (unbounded mpsc), so the UI thread never
        // blocks on a language server that is slow to drain its stdin.
        let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>();

        let client = Self {
            adapter,
            root: root_dir.map(Path::to_path_buf),
            out: out_tx.clone(),
            versions: versions.clone(),
            is_alive: is_alive.clone(),
            is_initialized: is_initialized.clone(),
            pending_opens: pending_opens.clone(),
            pending_changes: pending_changes.clone(),
            last_texts: last_texts.clone(),
            synced_texts: synced_texts.clone(),
            last_diagnostics: last_diagnostics.clone(),
            server_capabilities: server_capabilities.clone(),
            next_id: AtomicI64::new(100),
            pending: pending.clone(),
            child: child_arc,
        };

        // Send initialize request
        client.send_initialize(root_dir);

        // Writer thread: writes framed messages to the child's stdin and,
        // between messages, flushes coalesced didChange documents (debounced
        // ~120 ms). Since this is its own OS thread, a blocked `write_all`
        // (full pipe) costs the UI thread nothing.
        {
            let is_alive_w = is_alive.clone();
            let stdin_for_write = stdin_arc.clone();
            let pending_w = pending_changes.clone();
            let versions_w = versions.clone();
            let synced_w = synced_texts.clone();
            let caps_w = server_capabilities.clone();
            thread::spawn(move || {
                loop {
                    match out_rx.recv_timeout(Duration::from_millis(120)) {
                        Ok(bytes) => write_to_stdin(&stdin_for_write, &bytes),
                        Err(RecvTimeoutError::Timeout) => {
                            flush_pending_changes(
                                &stdin_for_write,
                                &pending_w,
                                &versions_w,
                                &synced_w,
                                &caps_w,
                            );
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                    if !*is_alive_w.lock().unwrap() {
                        flush_pending_changes(
                            &stdin_for_write,
                            &pending_w,
                            &versions_w,
                            &synced_w,
                            &caps_w,
                        );
                        break;
                    }
                }
            });
        }

        // Spawn stdout reader thread
        let lang_str = adapter.name.to_string();
        let root_for_reader = root_dir.map(Path::to_path_buf);
        let is_alive_clone = is_alive.clone();
        let is_init_clone = is_initialized.clone();
        let out_for_init = out_tx.clone();
        let pending_for_init = pending_opens.clone();
        let versions_for_init = versions.clone();
        let last_texts_r = last_texts.clone();
        let synced_r = synced_texts.clone();
        let last_diag_r = last_diagnostics.clone();
        let caps_r = server_capabilities.clone();
        let pending_r = pending.clone();

        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while *is_alive_clone.lock().unwrap() {
                match read_message(&mut reader) {
                    Ok(Some(mut msg)) => {
                        // Per the spec a request id is `integer | string`.
                        // Some servers (metals, and jdtls under load) use
                        // string ids, so a client that only reads integers
                        // silently drops their requests and deadlocks them.
                        let raw_id = msg.get("id").cloned().filter(|v| !v.is_null());
                        let id = raw_id.as_ref().and_then(Value::as_i64);
                        let method = msg
                            .get("method")
                            .and_then(Value::as_str)
                            .map(str::to_string);

                        // A server-initiated *request* (has both id and
                        // method) must always be answered. Zed registers
                        // handlers for these; the VS Code family of servers
                        // publishes no diagnostics at all until
                        // `workspace/configuration` is answered.
                        if let (Some(req_id), Some(m)) = (raw_id.clone(), method.as_deref()) {
                            handle_server_request(
                                adapter,
                                root_for_reader.as_deref(),
                                req_id,
                                m,
                                msg.get("params"),
                                &out_for_init,
                            );
                            continue;
                        }

                        match (id, method.as_deref()) {
                            // The `initialize` response (id 1, no method).
                            (Some(1), None) => {
                                // Remember what this server can do so providers
                                // can skip unsupported features.
                                *caps_r.lock().unwrap() = msg
                                    .get("result")
                                    .and_then(|r| r.get("capabilities"))
                                    .and_then(|c| {
                                        serde_json::from_value::<
                                            lsp_types::ServerCapabilities,
                                        >(c.clone())
                                        .ok()
                                    });

                                *is_init_clone.lock().unwrap() = true;

                                // Send initialized notification
                                let initialized = json!({
                                    "jsonrpc": "2.0",
                                    "method": "initialized",
                                    "params": InitializedParams {}
                                });
                                send_framed(&out_for_init, &initialized);

                                // Push our settings proactively. Servers that
                                // registered for `didChangeConfiguration`
                                // (yaml, eslint, json) apply validation
                                // settings from this rather than pulling
                                // them, so sending it is what turns their
                                // diagnostics on.
                                let settings =
                                    adapter.workspace_configuration("", root_for_reader.as_deref());
                                if !settings.is_null() {
                                    send_framed(
                                        &out_for_init,
                                        &json!({
                                            "jsonrpc": "2.0",
                                            "method": "workspace/didChangeConfiguration",
                                            "params": { "settings": settings }
                                        }),
                                    );
                                }

                                let _ = event_tx.try_send(LspEvent::Initialized {
                                    server: adapter.name.to_string(),
                                });

                                // Drain and send all pending did_open documents
                                let pendings: Vec<_> = {
                                    let mut guard = pending_for_init.lock().unwrap();
                                    guard.drain(..).collect()
                                };

                                for (path, lang, text) in pendings {
                                    if let Some(uri) = path_to_uri(&path) {
                                        versions_for_init.lock().unwrap().insert(path.clone(), 1);
                                        last_texts_r.lock().unwrap().insert(path.clone(), text.clone());
                                        synced_r.lock().unwrap().insert(path, text.clone());
                                        let params = DidOpenTextDocumentParams {
                                            text_document: TextDocumentItem {
                                                uri,
                                                language_id: lang,
                                                version: 1,
                                                text,
                                            },
                                        };
                                        let open_msg = json!({
                                            "jsonrpc": "2.0",
                                            "method": "textDocument/didOpen",
                                            "params": params
                                        });
                                        send_framed(&out_for_init, &open_msg);
                                    }
                                }
                            }
                            // A response to one of our requests.
                            (Some(req_id), None) => {
                                if let Some(tx) = pending_r.lock().unwrap().remove(&req_id) {
                                    let _ = tx.send(msg);
                                }
                            }
                            // Requests are handled above.
                            (Some(_), Some(_)) => {}
                            // A notification from the server.
                            (None, Some(_)) => {
                                handle_incoming_message(&mut msg, &event_tx, &last_diag_r);
                            }
                            (None, None) => {}
                        }
                    }
                    Ok(None) => {
                        break;
                    }
                    Err(e) => {
                        eprintln!("[LSP: {lang_str}] read error: {e}");
                        break;
                    }
                }
            }
            *is_alive_clone.lock().unwrap() = false;
            // Tell the workspace the process went away so it can respawn the
            // server for every open buffer. Without this, a crashed server
            // leaves the editor silently feature-less.
            let _ = event_tx.try_send(LspEvent::ServerExited {
                server: lang_str.clone(),
            });
        });

        Some(client)
    }

    pub fn is_alive(&self) -> bool {
        *self.is_alive.lock().unwrap()
    }

    /// True once the initialize handshake finished and the process is alive.
    pub fn is_ready(&self) -> bool {
        *self.is_initialized.lock().unwrap() && *self.is_alive.lock().unwrap()
    }

    /// The capabilities the server advertised in its initialize response.
    pub fn capabilities(&self) -> Option<lsp_types::ServerCapabilities> {
        self.server_capabilities.lock().unwrap().clone()
    }

    /// True when the server advertised a capability (or hasn't told us yet —
    /// requests are still safe because they no-op until initialized).
    pub fn supports(&self, pred: impl FnOnce(&lsp_types::ServerCapabilities) -> bool) -> bool {
        self.capabilities().as_ref().map(pred).unwrap_or(true)
    }

    /// The most recent text synced for `path`, used to answer requests that
    /// need line/character positions.
    pub fn last_text(&self, path: &Path) -> Option<String> {
        self.last_texts.lock().unwrap().get(path).cloned()
    }

    #[allow(deprecated)]
    fn send_initialize(&self, root_dir: Option<&Path>) {
        let mut params = InitializeParams::default();
        params.process_id = Some(std::process::id());
        params.root_uri = root_dir.and_then(path_to_uri);
        params.capabilities = ClientCapabilities {
            text_document: Some(TextDocumentClientCapabilities {
                publish_diagnostics: Some(PublishDiagnosticsClientCapabilities {
                    related_information: Some(true),
                    version_support: Some(true),
                    code_description_support: Some(true),
                    data_support: Some(true),
                    ..Default::default()
                }),
                synchronization: Some(TextDocumentSyncClientCapabilities {
                    dynamic_registration: Some(true),
                    will_save: Some(false),
                    will_save_wait_until: Some(false),
                    did_save: Some(true),
                }),
                completion: Some(lsp_types::CompletionClientCapabilities {
                    completion_item: Some(lsp_types::CompletionItemCapability {
                        snippet_support: Some(true),
                        documentation_format: Some(vec![lsp_types::MarkupKind::Markdown]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                hover: Some(lsp_types::HoverClientCapabilities {
                    content_format: Some(vec![lsp_types::MarkupKind::Markdown]),
                    ..Default::default()
                }),
                definition: Some(lsp_types::GotoCapability {
                    dynamic_registration: Some(true),
                    link_support: Some(true),
                }),
                code_action: Some(lsp_types::CodeActionClientCapabilities {
                    code_action_literal_support: Some(lsp_types::CodeActionLiteralSupport {
                        code_action_kind: lsp_types::CodeActionKindLiteralSupport {
                            value_set: vec![
                                lsp_types::CodeActionKind::QUICKFIX.as_str().to_string(),
                                lsp_types::CodeActionKind::REFACTOR.as_str().to_string(),
                                lsp_types::CodeActionKind::REFACTOR_EXTRACT.as_str().to_string(),
                                lsp_types::CodeActionKind::REFACTOR_INLINE.as_str().to_string(),
                                lsp_types::CodeActionKind::REFACTOR_REWRITE.as_str().to_string(),
                                lsp_types::CodeActionKind::SOURCE.as_str().to_string(),
                                lsp_types::CodeActionKind::SOURCE_ORGANIZE_IMPORTS.as_str().to_string(),
                            ],
                        },
                    }),
                    ..Default::default()
                }),
                formatting: Some(lsp_types::DocumentFormattingClientCapabilities {
                    dynamic_registration: Some(true),
                }),
                ..Default::default()
            }),
            // Workspace capabilities. `configuration: true` is what makes a
            // server send `workspace/configuration` at all — without it the
            // CSS/HTML/JSON/YAML/ESLint servers never ask for their settings,
            // fall back to their built-in defaults (validation off) and
            // publish nothing. Zed sets exactly these.
            workspace: Some(lsp_types::WorkspaceClientCapabilities {
                configuration: Some(true),
                did_change_configuration: Some(
                    lsp_types::DidChangeConfigurationClientCapabilities {
                        dynamic_registration: Some(true),
                    },
                ),
                did_change_watched_files: Some(
                    lsp_types::DidChangeWatchedFilesClientCapabilities {
                        dynamic_registration: Some(true),
                        relative_pattern_support: Some(false),
                    },
                ),
                workspace_folders: Some(true),
                apply_edit: Some(true),
                execute_command: Some(lsp_types::DynamicRegistrationClientCapabilities {
                    dynamic_registration: Some(true),
                }),
                symbol: Some(lsp_types::WorkspaceSymbolClientCapabilities {
                    dynamic_registration: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            // Servers gate progress reporting on this.
            window: Some(lsp_types::WindowClientCapabilities {
                work_done_progress: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        params.client_info = Some(lsp_types::ClientInfo {
            name: "ezicode".into(),
            version: Some("0.1.0".into()),
        });

        // Advertise the workspace folder too: ESLint and the JSON/YAML
        // servers resolve config files and `node_modules` relative to it.
        if let Some(root) = root_dir {
            if let Some(uri) = path_to_uri(root) {
                params.workspace_folders = Some(vec![lsp_types::WorkspaceFolder {
                    uri,
                    name: root
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                }]);
            }
        }

        // Per-server options come from the adapter (Zed's
        // `LspAdapter::initialization_options`).
        params.initialization_options = self.adapter.initialization_options(root_dir);

        let init_req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": params
        });

        self.send_payload(&init_req);
    }

    pub fn did_open(&self, path: &Path, lang: &str, text: &str) {
        // Translate our editor language id into the id the *protocol*
        // defines (Zed: `LspAdapter::language_ids`) — "tsx" becomes
        // "typescriptreact", "bash" becomes "shellscript".
        let language_id = self.adapter.language_id(lang).to_string();

        self.last_texts
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), text.to_string());
        // The didOpen text is exactly what the server will hold.
        self.synced_texts
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), text.to_string());

        if !*self.is_initialized.lock().unwrap() {
            // Queue until initialize handshake completes
            self.pending_opens
                .lock()
                .unwrap()
                .push((path.to_path_buf(), language_id, text.to_string()));
            return;
        }

        // Re-opening a document the server already knows about is a protocol
        // error; sync it instead.
        if self.versions.lock().unwrap().contains_key(path) {
            self.sync_document(path, text);
            return;
        }

        let Some(uri) = path_to_uri(path) else {
            return;
        };
        self.versions.lock().unwrap().insert(path.to_path_buf(), 1);

        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri,
                language_id,
                version: 1,
                text: text.to_string(),
            },
        };

        let msg = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": params
        });

        self.send_payload(&msg);
    }

    pub fn did_change(&self, path: &Path, text: String) {
        if !*self.is_initialized.lock().unwrap() {
            return;
        }
        // Coalesce: remember only the latest text per document. The writer
        // thread flushes pending changes as debounced full-document syncs,
        // so a fast typist costs one network round-trip every ~120 ms
        // instead of one whole-document serialization + write per keystroke.
        //
        // `text` arrives owned (the caller's single per-keystroke buffer
        // read); it is cloned once for `last_texts` and moved into
        // `pending_changes`, so a keystroke costs two copies total instead
        // of three.
        self.last_texts
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), text.clone());
        self.pending_changes
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), text);
    }

    pub fn did_close(&self, path: &Path) {
        let Some(uri) = path_to_uri(path) else {
            return;
        };
        self.versions.lock().unwrap().remove(path);
        self.last_texts.lock().unwrap().remove(path);
        self.synced_texts.lock().unwrap().remove(path);
        self.last_diagnostics.lock().unwrap().remove(path);
        // Drop any not-yet-flushed edit for the document so a coalesced
        // didChange can never race the didClose.
        self.pending_changes.lock().unwrap().remove(path);

        let params = DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier { uri },
        };

        let msg = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didClose",
            "params": params
        });

        self.send_payload(&msg);
    }

    /// `textDocument/didSave`. ESLint (`"run": "onType"` still re-lints on
    /// save), gopls and rust-analyzer run their heavier checks here, so a
    /// client that never sends it under-reports diagnostics.
    pub fn did_save(&self, path: &Path, text: &str) {
        if !self.is_ready() {
            return;
        }
        // Flush pending edits first so the server's copy matches the file.
        self.sync_document(path, text);

        let Some(uri) = path_to_uri(path) else {
            return;
        };
        let msg = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didSave",
            "params": {
                "textDocument": { "uri": uri },
                "text": text,
            }
        });
        self.send_payload(&msg);
    }

    /// Immediately sync `text` as the authoritative document, dropping any
    /// not-yet-flushed coalesced edit. Call before a request so the server's
    /// copy matches what the user sees.
    ///
    /// The sync is *incremental* when the server advertised it: we diff
    /// against the text the server last received and send one ranged edit
    /// covering just the difference. A full-document `didChange` is legal
    /// under any sync kind, so diffing degrades safely.
    pub fn sync_document(&self, path: &Path, text: &str) {
        if !self.is_ready() {
            return;
        }
        let Some(uri) = path_to_uri(path) else {
            return;
        };
        self.pending_changes.lock().unwrap().remove(path);
        // Providers (code actions) read `last_texts` as the *editor's* text.
        self.last_texts
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), text.to_string());

        let change = next_change_for(
            &self.synced_texts,
            path,
            text,
            server_wants_incremental(&self.server_capabilities.lock().unwrap()),
        );
        let Some(change) = change else {
            // Server is already up to date; re-announcing would force it to
            // re-analyse the document for no reason.
            return;
        };

        let version = {
            let mut versions_guard = self.versions.lock().unwrap();
            let v = versions_guard.entry(path.to_path_buf()).or_insert(0);
            *v += 1;
            *v
        };
        let params = DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier { uri, version },
            content_changes: vec![change],
        };
        let msg = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": params
        });
        self.send_payload(&msg);
    }

    /// Send a JSON-RPC request and block (up to `timeout`) for its response.
    /// Returns the `result` field, or `None` on timeout / server error /
    /// disconnect. Safe to call from background threads; never call from the
    /// UI thread.
    pub fn request(&self, method: &str, params: Value, timeout: Duration) -> Option<Value> {
        if !self.is_ready() {
            return None;
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel::<Value>();
        self.pending.lock().unwrap().insert(id, tx);
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        self.send_payload(&msg);
        match rx.recv_timeout(timeout) {
            Ok(resp) => resp.get("result").cloned(),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                None
            }
        }
    }

    /// [`Self::sync_document`] followed by [`Self::request`] with the
    /// default timeout. The server's answer is guaranteed to be computed
    /// against exactly `text`.
    pub fn request_with_text(
        &self,
        path: &Path,
        text: &str,
        method: &str,
        params: Value,
    ) -> Option<Value> {
        self.sync_document(path, text);
        self.request(method, params, REQUEST_TIMEOUT)
    }

    /// `textDocument/formatting` for the whole document.
    pub fn format_document(&self, path: &Path, text: &str) -> Option<Vec<lsp_types::TextEdit>> {
        if !self.supports(|c| c.document_formatting_provider.is_some()) {
            return None;
        }
        let uri = path_to_uri(path)?;
        let params = DocumentFormattingParams {
            text_document: TextDocumentIdentifier::new(uri),
            options: FormattingOptions {
                tab_size: 4,
                insert_spaces: true,
                properties: Default::default(),
                trim_trailing_whitespace: None,
                insert_final_newline: None,
                trim_final_newlines: None,
            },
            work_done_progress_params: Default::default(),
        };
        let params = serde_json::to_value(params).ok()?;
        let response = self.request_with_text(path, text, "textDocument/formatting", params)?;
        serde_json::from_value::<Vec<lsp_types::TextEdit>>(response).ok()
    }

    fn send_payload(&self, val: &Value) {
        send_framed(&self.out, val);
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        // Best-effort graceful shutdown: the writer thread may flush these
        // before the child is killed.
        *self.is_alive.lock().unwrap() = false;
        let shutdown = json!({"jsonrpc": "2.0", "id": 999_999, "method": "shutdown", "params": null});
        if let Some(bytes) = frame_payload(&shutdown) {
            let _ = self.out.send(bytes);
        }
        let exit = json!({"jsonrpc": "2.0", "method": "exit"});
        if let Some(bytes) = frame_payload(&exit) {
            let _ = self.out.send(bytes);
        }
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
    }
}

/// Attach the standard LSP provider set (completions, hover, go-to-definition,
/// code actions) to an editor. Call once per editor after its server is
/// ensured; the providers talk to `client` and identify documents by `path`.
pub fn attach_lsp_providers(state: &mut InputState, client: Arc<LspClient>, path: PathBuf) {
    state.lsp.completion_provider = Some(Rc::new(LspCompletionProvider {
        client: client.clone(),
        path: path.clone(),
    }));
    state.lsp.hover_provider = Some(Rc::new(LspHoverProvider {
        client: client.clone(),
        path: path.clone(),
    }));
    state.lsp.definition_provider = Some(Rc::new(LspDefinitionProvider {
        client: client.clone(),
        path: path.clone(),
    }));
    state.lsp.code_action_providers = vec![Rc::new(LspCodeActionProvider { client, path })];
}

/// Characters that fire a completion request while typing. Mirrors the
/// trigger set of VS Code / Zed: word characters plus common punctuation
/// that usually starts a member access or argument list.
pub fn is_completion_trigger_char(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(
            c,
            '_' | '.' | '$' | ':' | '/' | '<' | '"' | '#' | '@' | '-' | '>' | '(' | '[' | '{'
        )
}

// -- Providers --------------------------------------------------------------

/// `textDocument/completion` + inline (ghost text) completions.
pub struct LspCompletionProvider {
    client: Arc<LspClient>,
    path: PathBuf,
}

impl CompletionProvider for LspCompletionProvider {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        trigger: CompletionContext,
        _window: &mut Window,
        cx: &mut Context<InputState>,
    ) -> Task<anyhow::Result<CompletionResponse>> {
        if !self.client.supports(|c| c.completion_provider.is_some()) {
            return Task::ready(Ok(CompletionResponse::Array(vec![])));
        }
        let client = self.client.clone();
        let path = self.path.clone();
        let text_str = text.to_string();
        let position = text.offset_to_position(offset);

        // The library hands us the whole query text typed since the menu
        // opened; the protocol wants a single trigger character, so only
        // forward it when it is exactly one char.
        let trigger_character = trigger.trigger_character.filter(|c| c.chars().count() == 1);
        let trigger_kind = if trigger_character.is_some() {
            trigger.trigger_kind
        } else {
            CompletionTriggerKind::INVOKED
        };

        cx.background_spawn(async move {
            let Some(uri) = path_to_uri(&path) else {
                return Ok(CompletionResponse::Array(vec![]));
            };
            let params = CompletionParams {
                text_document_position: TextDocumentPositionParams::new(
                    TextDocumentIdentifier::new(uri),
                    position,
                ),
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
                context: Some(CompletionContext {
                    trigger_kind,
                    trigger_character,
                }),
            };
            let Ok(params) = serde_json::to_value(params) else {
                return Ok(CompletionResponse::Array(vec![]));
            };
            let response =
                client.request_with_text(&path, &text_str, "textDocument/completion", params);
            Ok(response
                .and_then(|v| serde_json::from_value::<CompletionResponse>(v).ok())
                .unwrap_or(CompletionResponse::Array(vec![])))
        })
    }

    fn is_completion_trigger(
        &self,
        _offset: usize,
        new_text: &str,
        _cx: &mut Context<InputState>,
    ) -> bool {
        new_text.chars().next().map(is_completion_trigger_char).unwrap_or(false)
    }
}

/// `textDocument/hover` — the library shows the result in a popover.
pub struct LspHoverProvider {
    client: Arc<LspClient>,
    path: PathBuf,
}

impl HoverProvider for LspHoverProvider {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Option<Hover>>> {
        if !self.client.supports(|c| c.hover_provider.is_some()) {
            return Task::ready(Ok(None));
        }
        let client = self.client.clone();
        let path = self.path.clone();
        let text_str = text.to_string();
        let position = text.offset_to_position(offset);

        cx.background_spawn(async move {
            let Some(uri) = path_to_uri(&path) else {
                return Ok(None);
            };
            let params = TextDocumentPositionParams::new(TextDocumentIdentifier::new(uri), position);
            let Ok(params) = serde_json::to_value(params) else {
                return Ok(None);
            };
            let response = client.request_with_text(&path, &text_str, "textDocument/hover", params);
            Ok(response
                .and_then(|v| serde_json::from_value::<Option<Hover>>(v).ok())
                .flatten())
        })
    }
}

/// `textDocument/definition` — ctrl-hover underlines the symbol and F12
/// jumps (the library renders the link highlight and handles the jump).
pub struct LspDefinitionProvider {
    client: Arc<LspClient>,
    path: PathBuf,
}

impl DefinitionProvider for LspDefinitionProvider {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Vec<LocationLink>>> {
        if !self.client.supports(|c| c.definition_provider.is_some()) {
            return Task::ready(Ok(vec![]));
        }
        let client = self.client.clone();
        let path = self.path.clone();
        let text_str = text.to_string();
        let position = text.offset_to_position(offset);

        cx.background_spawn(async move {
            let Some(uri) = path_to_uri(&path) else {
                return Ok(vec![]);
            };
            let params = TextDocumentPositionParams::new(TextDocumentIdentifier::new(uri), position);
            let Ok(params) = serde_json::to_value(params) else {
                return Ok(vec![]);
            };
            let response = client.request_with_text(&path, &text_str, "textDocument/definition", params);
            let locations = response
                .and_then(|v| serde_json::from_value::<GotoDefinitionResponse>(v).ok())
                .map(|r| match r {
                    GotoDefinitionResponse::Scalar(loc) => vec![location_to_link(loc)],
                    GotoDefinitionResponse::Array(locs) => {
                        locs.into_iter().map(location_to_link).collect()
                    }
                    GotoDefinitionResponse::Link(links) => links,
                })
                .unwrap_or_default();
            Ok(locations)
        })
    }
}

/// `textDocument/codeAction` + `workspace/executeCommand`.
pub struct LspCodeActionProvider {
    client: Arc<LspClient>,
    path: PathBuf,
}

impl CodeActionProvider for LspCodeActionProvider {
    fn id(&self) -> SharedString {
        "LSP".into()
    }

    fn code_actions(
        &self,
        _state: Entity<InputState>,
        range: StdRange<usize>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Vec<CodeAction>>> {
        if !self.client.supports(|c| c.code_action_provider.is_some()) {
            return Task::ready(Ok(vec![]));
        }
        let client = self.client.clone();
        let path = self.path.clone();

        cx.background_spawn(async move {
            // The byte range refers to the text we last synced; rebuild the
            // rope so we can convert it to line/character positions.
            let Some(text) = client.last_text(&path) else {
                return Ok(vec![]);
            };
            let rope = Rope::from_str(&text);
            let start = rope.offset_to_position(range.start);
            let end = rope.offset_to_position(range.end);

            let Some(uri) = path_to_uri(&path) else {
                return Ok(vec![]);
            };
            let Some(params) = serde_json::to_value(CodeActionParams {
                text_document: TextDocumentIdentifier::new(uri),
                range: lsp_types::Range::new(start, end),
                context: CodeActionContext {
                    diagnostics: client
                        .last_diagnostics
                        .lock()
                        .unwrap()
                        .get(&path)
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|d| {
                            // Keep only diagnostics overlapping the range.
                            !(d.range.end <= start || d.range.start >= end)
                        })
                        .collect(),
                    only: None,
                    trigger_kind: Some(CodeActionTriggerKind::INVOKED),
                },
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            })
            .ok()
            else {
                return Ok(vec![]);
            };

            let response =
                client.request_with_text(&path, &text, "textDocument/codeAction", params);
            let actions = response
                .and_then(|v| serde_json::from_value::<CodeActionResponse>(v).ok())
                .unwrap_or_default()
                .into_iter()
                .map(|item| match item {
                    CodeActionOrCommand::CodeAction(action) => action,
                    CodeActionOrCommand::Command(command) => CodeAction {
                        title: command.title.clone(),
                        kind: None,
                        diagnostics: None,
                        edit: None,
                        command: Some(command),
                        is_preferred: None,
                        disabled: None,
                        data: None,
                    },
                })
                .collect();
            Ok(actions)
        })
    }

    fn perform_code_action(
        &self,
        state: Entity<InputState>,
        action: CodeAction,
        _push_to_history: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<()>> {
        let client = self.client.clone();
        let path = self.path.clone();

        // Apply the action's workspace edits to the current document (the
        // library only has one document per editor; multi-file edits like
        // renames touch the other files on the next open).
        if let Some(edit) = action.edit {
            if let (Some(uri), Some(changes)) = (path_to_uri(&path), edit.changes) {
                if let Some((_, text_edits)) =
                    changes.iter().find(|(u, _)| u.as_str() == uri.as_str())
                {
                    let text_edits = text_edits.clone();
                    let state = state.downgrade();
                    let _ = window.spawn(cx, async move |cx| {
                        if let Some(state) = state.upgrade() {
                            let _ = state.update_in(cx, |state, window, cx| {
                                state.apply_lsp_edits(&text_edits, window, cx);
                            });
                        }
                    });
                }
            }
        }

        // Execute the action's command on the server.
        if let Some(command) = action.command {
            let command_name = command.command;
            let arguments = command.arguments.unwrap_or_default();
            return cx.background_spawn(async move {
                let params = json!({ "command": command_name, "arguments": arguments });
                let _ = client.request("workspace/executeCommand", params, REQUEST_TIMEOUT);
                Ok(())
            });
        }

        Task::ready(Ok(()))
    }
}

fn location_to_link(loc: Location) -> LocationLink {
    LocationLink {
        target_uri: loc.uri,
        target_range: loc.range,
        target_selection_range: loc.range,
        origin_selection_range: None,
    }
}

/// Serialize `val` as one framed (Content-Length headed) JSON-RPC message.
/// `None` when serialization fails.
fn frame_payload(val: &Value) -> Option<Vec<u8>> {
    let json_str = serde_json::to_string(val).ok()?;
    let body = json_str.as_bytes();
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut bytes = Vec::with_capacity(header.len() + body.len());
    bytes.extend_from_slice(header.as_bytes());
    bytes.extend_from_slice(body);
    Some(bytes)
}

/// Non-blocking enqueue of one framed message onto the writer channel.
fn send_framed(tx: &mpsc::Sender<Vec<u8>>, val: &Value) {
    if let Some(bytes) = frame_payload(val) {
        let _ = tx.send(bytes);
    }
}

/// Write framed bytes to the child's stdin (writer thread only).
fn write_to_stdin(stdin_arc: &Arc<Mutex<Option<ChildStdin>>>, bytes: &[u8]) {
    let mut stdin_guard = stdin_arc.lock().unwrap();
    if let Some(stdin) = stdin_guard.as_mut() {
        let _ = stdin.write_all(bytes);
        let _ = stdin.flush();
    }
}

/// Flush every coalesced `didChange` document as an incremental (ranged)
/// edit with a monotonically increasing version. Runs on the writer thread,
/// so any pipe backpressure blocks a background thread, never the UI.
fn flush_pending_changes(
    stdin_arc: &Arc<Mutex<Option<ChildStdin>>>,
    pending: &Mutex<HashMap<PathBuf, String>>,
    versions: &Mutex<HashMap<PathBuf, i32>>,
    synced_texts: &Mutex<HashMap<PathBuf, String>>,
    server_capabilities: &Mutex<Option<lsp_types::ServerCapabilities>>,
) {
    let items: Vec<(PathBuf, String)> = {
        let mut guard = pending.lock().unwrap();
        guard.drain().collect()
    };
    if items.is_empty() {
        return;
    }
    let incremental = server_wants_incremental(&server_capabilities.lock().unwrap());
    for (path, text) in items {
        // The document was closed before the flush — drop the stale edit.
        let doc_open = versions.lock().unwrap().contains_key(&path);
        if !doc_open {
            continue;
        }
        let Some(change) = next_change_for(synced_texts, &path, &text, incremental) else {
            continue;
        };
        let Some(uri) = path_to_uri(&path) else {
            continue;
        };
        let version = {
            let mut versions_guard = versions.lock().unwrap();
            let v = versions_guard.entry(path).or_insert(1);
            *v += 1;
            *v
        };
        let params = DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier { uri, version },
            content_changes: vec![change],
        };
        let msg = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": params
        });
        if let Some(bytes) = frame_payload(&msg) {
            write_to_stdin(stdin_arc, &bytes);
        }
    }
}

/// Diff the server's current copy of `path` against `text`, record `text` as
/// the new server copy, and return the change event to send — `None` when
/// the server is already up to date. The mutex is held across the diff so a
/// concurrent `flush_pending_changes` and `sync_document` can never compute
/// from the same base and apply the same change twice.
fn next_change_for(
    synced: &Mutex<HashMap<PathBuf, String>>,
    path: &Path,
    text: &str,
    incremental: bool,
) -> Option<TextDocumentContentChangeEvent> {
    let mut guard = synced.lock().unwrap();
    let prev = guard.insert(path.to_path_buf(), text.to_string());
    if prev.as_deref() == Some(text) {
        return None;
    }
    Some(content_change_for(&prev, text, incremental))
}

/// Build a single `TextDocumentContentChangeEvent` that transforms `prev`
/// (the text the server currently holds, or `None` for a fresh document)
/// into `text`: a ranged edit covering just the difference when we know the
/// old text and the server accepts incremental sync, a full replacement
/// otherwise (always legal under any sync kind).
fn content_change_for(
    prev: &Option<String>,
    text: &str,
    incremental: bool,
) -> TextDocumentContentChangeEvent {
    match (incremental, prev) {
        (true, Some(prev)) => {
            let (start, end, inserted) = diff_edit(prev, text);
            if start == end && inserted.is_empty() {
                // Identical texts (defensive; callers skip this case).
                return TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: text.to_string(),
                };
            }
            TextDocumentContentChangeEvent {
                range: Some(lsp_types::Range::new(
                    offset_to_position(prev, start),
                    offset_to_position(prev, end),
                )),
                range_length: None,
                text: inserted.to_string(),
            }
        }
        _ => TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: text.to_string(),
        },
    }
}

/// True when the server advertised incremental (`didChange`) sync. The sync
/// kind is a *server* capability; before the initialize response arrives we
/// conservatively send full documents.
fn server_wants_incremental(caps: &Option<lsp_types::ServerCapabilities>) -> bool {
    use lsp_types::TextDocumentSyncCapability;
    const INCREMENTAL: lsp_types::TextDocumentSyncKind =
        lsp_types::TextDocumentSyncKind::INCREMENTAL;
    match caps.as_ref().and_then(|c| c.text_document_sync.as_ref()) {
        Some(TextDocumentSyncCapability::Kind(kind)) => *kind == INCREMENTAL,
        Some(TextDocumentSyncCapability::Options(options)) => options.change == Some(INCREMENTAL),
        None => false,
    }
}

/// Diff `old` → `new` into one edit: `(start, end)` are byte offsets into
/// `old` whose contents are replaced by the returned slice of `new`.
///
/// The common prefix/suffix search works on bytes but is pulled back to
/// `char` boundaries, so the offsets are always valid to convert to LSP
/// positions. If nothing changed the result replaces an empty range with an
/// empty string.
fn diff_edit<'a>(old: &'a str, new: &'a str) -> (usize, usize, &'a str) {
    if old == new {
        return (0, 0, "");
    }
    let ob = old.as_bytes();
    let nb = new.as_bytes();

    // Common prefix, clamped to a char boundary.
    let mut p = 0;
    let max = ob.len().min(nb.len());
    while p < max && ob[p] == nb[p] {
        p += 1;
    }
    while p > 0 && (!old.is_char_boundary(p) || !new.is_char_boundary(p)) {
        p -= 1;
    }

    // Common suffix, not allowed to reach into the prefix.
    let mut s = 0;
    while s < ob.len() - p
        && s < nb.len() - p
        && ob[ob.len() - 1 - s] == nb[nb.len() - 1 - s]
    {
        s += 1;
    }
    while s > 0
        && (!old.is_char_boundary(ob.len() - s) || !new.is_char_boundary(nb.len() - s))
    {
        s -= 1;
    }

    (p, ob.len() - s, &new[p..nb.len() - s])
}

/// Convert a byte offset into an LSP `Position` (line + UTF-16 code units on
/// the line). `byte_offset` must land on a char boundary; offsets past the
/// end clamp to the end of the text.
fn offset_to_position(text: &str, byte_offset: usize) -> lsp_types::Position {
    let mut line: u32 = 0;
    let mut col: u32 = 0;
    for (i, ch) in text.char_indices() {
        if i >= byte_offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += ch.len_utf16() as u32;
        }
    }
    lsp_types::Position::new(line, col)
}

pub fn path_to_uri(path: &Path) -> Option<Uri> {
    let url = url::Url::from_file_path(path).ok()?;
    url.to_string().parse().ok()
}

pub fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let url = url::Url::parse(uri.as_str()).ok()?;
    url.to_file_path().ok()
}

pub fn paths_match(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
    } else {
        a == b
    }
}

pub fn find_binary_on_path(binary: &str) -> Option<PathBuf> {
    const WINDOWS_EXTS: [&str; 3] = [".cmd", ".exe", ".bat"];
    let candidates: Vec<String> = if cfg!(windows) && !WINDOWS_EXTS.iter().any(|e| binary.ends_with(e)) {
        WINDOWS_EXTS.iter().map(|e| format!("{binary}{e}")).collect()
    } else {
        vec![binary.to_string()]
    };
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            candidates.iter().find_map(|name| {
                let p = dir.join(name);
                if p.is_file() {
                    Some(p)
                } else {
                    None
                }
            })
        })
    })
}

fn read_message<R: BufRead>(reader: &mut R) -> std::io::Result<Option<Value>> {
    let mut content_length = None;

    loop {
        let mut line = String::new();
        let bytes_read = reader.read_line(&mut line)?;
        if bytes_read == 0 {
            return Ok(None);
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }

        if let Some(len_str) = trimmed.strip_prefix("Content-Length:") {
            if let Ok(len) = len_str.trim().parse::<usize>() {
                content_length = Some(len);
            }
        }
    }

    let Some(length) = content_length else {
        return Ok(None);
    };

    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;

    let val = serde_json::from_slice(&body).ok();
    Ok(val)
}

/// Answer a server-initiated request.
///
/// Zed registers handlers for these on every server it starts
/// (`crates/project/src/lsp_store.rs`). Answering them is not optional:
///
/// * **`workspace/configuration`** — the VS Code-derived servers (CSS, HTML,
///   JSON, YAML, ESLint) ask for their settings immediately after
///   `initialized` and **publish no diagnostics until they get a reply**.
///   Replying "method not found" is the single most common reason a
///   hand-rolled client shows errors for TypeScript but stays silent for CSS
///   and HTML.
/// * **`client/registerCapability`** — servers register dynamic capabilities
///   here (tsserver registers most of its features this way). It must be
///   acknowledged or the server may never finish starting.
/// * **`window/workDoneProgress/create`** — must be acknowledged before the
///   server will emit progress notifications.
/// * **`workspace/applyEdit`** — must be answered so the server doesn't block
///   after a rename/quick-fix; we report `applied: false` because applying a
///   multi-file edit needs the buffer store.
///
/// Anything genuinely unknown still gets `-32601`, per the spec.
fn handle_server_request(
    adapter: &'static super::adapter::ServerAdapter,
    root: Option<&Path>,
    id: Value,
    method: &str,
    params: Option<&Value>,
    out: &mpsc::Sender<Vec<u8>>,
) {
    let result: Option<Value> = match method {
        "workspace/configuration" => {
            // Reply with one settings object per requested item, in order.
            let items = params
                .and_then(|p| p.get("items"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let configs: Vec<Value> = if items.is_empty() {
                vec![adapter.workspace_configuration("", root)]
            } else {
                items
                    .iter()
                    .map(|item| {
                        let section = item
                            .get("section")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        adapter.workspace_configuration(section, root)
                    })
                    .collect()
            };
            Some(Value::Array(configs))
        }
        // Acknowledge with a null result.
        "client/registerCapability"
        | "client/unregisterCapability"
        | "window/workDoneProgress/create" => Some(Value::Null),
        "workspace/applyEdit" => Some(json!({ "applied": false })),
        "workspace/semanticTokens/refresh"
        | "workspace/inlayHint/refresh"
        | "workspace/codeLens/refresh"
        | "workspace/diagnostic/refresh" => Some(Value::Null),
        _ => None,
    };

    let response = match result {
        Some(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        None => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": format!("method not found: {method}") }
        }),
    };
    send_framed(out, &response);
}

fn handle_incoming_message(
    msg: &mut Value,
    event_tx: &async_channel::Sender<LspEvent>,
    last_diag: &Mutex<HashMap<PathBuf, Vec<lsp_types::Diagnostic>>>,
) {
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return;
    };

    if method == "textDocument/publishDiagnostics" {
        // `params` is taken (moved) out of `msg` instead of cloned — the
        // JSON body of a diagnostics push is the heaviest per-message cost.
        if let Some(params) = msg.get_mut("params") {
            if let Ok(mut pub_diag) =
                serde_json::from_value::<PublishDiagnosticsParams>(std::mem::take(params))
            {
                if let Some(path) = uri_to_path(&pub_diag.uri) {
                    last_diag
                        .lock()
                        .unwrap()
                        .insert(path.clone(), pub_diag.diagnostics.clone());
                    for diag in &mut pub_diag.diagnostics {
                        let source_str = diag.source.as_deref().unwrap_or("typescript");
                        let code_str = match &diag.code {
                            Some(lsp_types::NumberOrString::Number(n)) => format!(" ({n})"),
                            Some(lsp_types::NumberOrString::String(s)) => format!(" ({s})"),
                            None => String::new(),
                        };
                        diag.message = format!("`{source_str}{code_str}`\n\n{}", diag.message);
                    }
                    let _ = event_tx.try_send(LspEvent::Diagnostics {
                        path,
                        diagnostics: pub_diag.diagnostics,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uri_path_roundtrip() {
        let path = if cfg!(windows) {
            PathBuf::from(r"C:\Users\test\project\app.ts")
        } else {
            PathBuf::from("/home/test/project/app.ts")
        };
        let uri = path_to_uri(&path).expect("valid uri");
        let roundtrip = uri_to_path(&uri).expect("valid path");
        assert!(paths_match(&path, &roundtrip));
    }

    #[test]
    fn test_read_message_framing() {
        let payload = r#"{"jsonrpc":"2.0","method":"test","params":{}}"#;
        let framed = format!("Content-Length: {}\r\n\r\n{}", payload.len(), payload);
        let mut cursor = std::io::Cursor::new(framed.into_bytes());
        let msg = read_message(&mut cursor).expect("read ok").expect("some msg");
        assert_eq!(msg["method"], "test");
    }

    #[test]
    fn test_trigger_chars() {
        assert!(is_completion_trigger_char('a'));
        assert!(is_completion_trigger_char('_'));
        assert!(is_completion_trigger_char('.'));
        assert!(!is_completion_trigger_char(' '));
        assert!(!is_completion_trigger_char(';'));
    }

    /// Collect the framed messages a `handle_server_request` call produced.
    fn drain(rx: &mpsc::Receiver<Vec<u8>>) -> Vec<Value> {
        let mut out = Vec::new();
        while let Ok(bytes) = rx.try_recv() {
            let text = String::from_utf8_lossy(&bytes);
            if let Some(idx) = text.find("\r\n\r\n") {
                if let Ok(v) = serde_json::from_str::<Value>(&text[idx + 4..]) {
                    out.push(v);
                }
            }
        }
        out
    }

    /// The regression this whole change exists to prevent: replying
    /// "method not found" to `workspace/configuration` makes the CSS, HTML,
    /// JSON, YAML and ESLint servers publish nothing at all.
    #[test]
    fn workspace_configuration_is_answered_per_item() {
        let css = super::super::adapter::adapter_by_name("vscode-css-language-server").unwrap();
        let (tx, rx) = mpsc::channel();

        handle_server_request(
            css,
            None,
            json!(7),
            "workspace/configuration",
            Some(&json!({ "items": [{ "section": "css" }, { "section": "scss" }] })),
            &tx,
        );

        let sent = drain(&rx);
        assert_eq!(sent.len(), 1);
        let reply = &sent[0];
        assert_eq!(reply["id"], 7);
        assert!(reply.get("error").is_none(), "must not be an error reply");

        // One config per requested item, in order, with validation enabled.
        let result = reply["result"].as_array().expect("array result");
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["validate"], true);
        assert_eq!(result[1]["validate"], true);
    }

    /// Request ids may be strings; the reply must echo the id unchanged.
    #[test]
    fn string_request_ids_are_echoed_verbatim() {
        let ts = super::super::adapter::adapter_by_name("typescript-language-server").unwrap();
        let (tx, rx) = mpsc::channel();

        handle_server_request(
            ts,
            None,
            json!("req-abc"),
            "client/registerCapability",
            Some(&json!({ "registrations": [] })),
            &tx,
        );

        let sent = drain(&rx);
        assert_eq!(sent[0]["id"], "req-abc");
        assert!(sent[0].get("error").is_none());
        assert!(sent[0]["result"].is_null());
    }

    /// Incremental sync: the edit must cover exactly the changed bytes and
    /// apply cleanly on the server's copy.
    #[test]
    fn diff_edit_produces_minimal_ranged_edits() {
        // Pure insertion in the middle.
        let (s, e, ins) = diff_edit("hello world", "hello big world");
        assert_eq!((s, e, ins), (6, 6, "big "));
        // Pure deletion.
        let (s, e, ins) = diff_edit("hello big world", "hello world");
        assert_eq!((s, e, ins), (6, 10, ""));
        // Replacement.
        let (s, e, ins) = diff_edit("hello world", "hello there");
        assert_eq!((s, e, ins), (6, 11, "there"));
        // Append.
        let (s, e, ins) = diff_edit("abc", "abcdef");
        assert_eq!((s, e, ins), (3, 3, "def"));
        // Truncate.
        let (s, e, ins) = diff_edit("abcdef", "abc");
        assert_eq!((s, e, ins), (3, 6, ""));
        // No change.
        assert_eq!(diff_edit("same", "same"), (0, 0, ""));
    }

    /// Multibyte characters must not end up split across the edit range:
    /// the offsets are pulled back to char boundaries.
    #[test]
    fn diff_edit_stays_on_char_boundaries() {
        // é is two bytes; the differing byte sits inside the char.
        let (s, e, ins) = diff_edit("café", "café!"); // pure append after é
        let mut applied = "café".to_string();
        applied.replace_range(s..e, ins);
        assert_eq!(applied, "café!");
        assert!("café".is_char_boundary(s) && "café".is_char_boundary(e));

        let (s, e, ins) = diff_edit("éa", "e̶a"); // different lead char
        let mut applied = "éa".to_string();
        applied.replace_range(s..e, ins);
        assert_eq!(applied, "e̶a");
    }

    /// LSP positions are line + UTF-16 code units — the classic trap is an
    /// astral-plane char (emoji) counting as 2, and CRLF splitting lines.
    #[test]
    fn offset_to_position_counts_utf16_units() {
        let text = "ab\ncd😀\nef";
        // Start of line 2 ("ef") — 😀 is 4 UTF-8 bytes but 2 UTF-16 units.
        let off = "ab\n".len() + "cd".len() + 4; // past the emoji
        let pos = offset_to_position(text, off);
        assert_eq!((pos.line, pos.character), (1, 4));
        // Start of line 3.
        let pos = offset_to_position(text, text.len() - 2);
        assert_eq!((pos.line, pos.character), (2, 0));
        // End of text.
        let pos = offset_to_position(text, text.len());
        assert_eq!((pos.line, pos.character), (2, 2));
    }

    /// The full chain: old server text + new buffer text → one change event
    /// whose range refers to the OLD text (as the spec requires) and which,
    /// when applied, reproduces the new text exactly.
    #[test]
    fn content_change_ranges_reference_the_old_text() {
        let prev = "let x = 1;\nlet y = 2;\n".to_string();
        let next = "let x = 1;\nlet y = 42;\n";
        let change = content_change_for(&Some(prev.clone()), next, true);
        let range = change.range.expect("should be a ranged edit");
        assert_eq!(range.start.line, 1);
        assert_eq!(range.end.line, 1);

        // Apply the ranged edit to the old text; the result must be `next`.
        let start = position_to_offset(&prev, range.start);
        let end = position_to_offset(&prev, range.end);
        let mut applied = prev.clone();
        applied.replace_range(start..end, &change.text);
        assert_eq!(applied, next);

        // No previous text (fresh doc) → full replacement, no range.
        let change = content_change_for(&None, "fresh", true);
        assert!(change.range.is_none());
        assert_eq!(change.text, "fresh");

        // A full-sync server always gets the whole document.
        let change = content_change_for(&Some(prev), next, false);
        assert!(change.range.is_none());
        assert_eq!(change.text, next);
    }

    /// Inverse of `offset_to_position`, for verifying edits in tests.
    fn position_to_offset(text: &str, pos: lsp_types::Position) -> usize {
        let mut line = 0u32;
        for (i, ch) in text.char_indices() {
            if line == pos.line {
                let col = text[..i]
                    .chars()
                    .rev()
                    .take_while(|c| *c != '\n')
                    .count() as u32;
                return i + ((pos.character - col) as usize);
            }
            if ch == '\n' {
                line += 1;
            }
        }
        text.len()
    }

    /// The sync kind is a server capability; both spellings the spec allows
    /// must be recognised.
    #[test]
    fn incremental_sync_kind_is_detected() {
        use lsp_types::{ServerCapabilities, TextDocumentSyncCapability};
        assert!(!server_wants_incremental(&None));
        assert!(!server_wants_incremental(&Some(ServerCapabilities::default())));
        assert!(server_wants_incremental(&Some(ServerCapabilities {
            text_document_sync: Some(TextDocumentSyncCapability::Kind(
                lsp_types::TextDocumentSyncKind::INCREMENTAL
            )),
            ..Default::default()
        })));
        assert!(server_wants_incremental(&Some(ServerCapabilities {
            text_document_sync: Some(TextDocumentSyncCapability::Options(
                lsp_types::TextDocumentSyncOptions {
                    change: Some(lsp_types::TextDocumentSyncKind::INCREMENTAL),
                    ..Default::default()
                }
            )),
            ..Default::default()
        })));
        assert!(!server_wants_incremental(&Some(ServerCapabilities {
            text_document_sync: Some(TextDocumentSyncCapability::Kind(
                lsp_types::TextDocumentSyncKind::FULL
            )),
            ..Default::default()
        })));
    }

    /// Regression: the debounced flush must diff against what the *server*
    /// holds, not what the editor holds. When the diff base was the editor
    /// text (already updated by `did_change`), every flushed edit diffed
    /// against itself and was silently skipped — the server stayed on the
    /// didOpen text and stopped publishing diagnostics.
    #[test]
    fn flush_diffs_against_the_server_copy_not_the_editor_copy() {
        let synced: Mutex<HashMap<PathBuf, String>> = Mutex::new(HashMap::new());
        let path = PathBuf::from("C:/proj/a.ts");
        // did_open: the server now holds "abc".
        next_change_for(&synced, &path, "abc", false);

        // Editor changes to "abcd" (did_change updates the editor map, not
        // `synced`). The first flush must produce a real ranged edit…
        let change = next_change_for(&synced, &path, "abcd", true).unwrap();
        let range = change.range.unwrap();
        assert_eq!(range.start.character, 3);
        assert_eq!(change.text, "d");

        // …and a repeat flush of the same text must be a no-op, not a
        // second (corrupting) application.
        assert!(next_change_for(&synced, &path, "abcd", true).is_none());
    }

    /// Genuinely unknown methods still get a spec-compliant error, so a
    /// server never blocks waiting on us.
    #[test]
    fn unknown_server_requests_get_method_not_found() {
        let ts = super::super::adapter::adapter_by_name("typescript-language-server").unwrap();
        let (tx, rx) = mpsc::channel();
        handle_server_request(ts, None, json!(3), "some/unknownMethod", None, &tx);
        let sent = drain(&rx);
        assert_eq!(sent[0]["error"]["code"], -32601);
    }

    /// Frames must be byte-length prefixed, not char-length — a diagnostic
    /// containing non-ASCII would otherwise desynchronise the stream.
    #[test]
    fn framing_uses_byte_length_for_non_ascii() {
        let msg = json!({ "jsonrpc": "2.0", "method": "note", "params": { "s": "héllo — ✓" } });
        let bytes = frame_payload(&msg).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        let header_end = text.find("\r\n\r\n").unwrap();
        let declared: usize = text[..header_end]
            .trim_start_matches("Content-Length:")
            .trim()
            .parse()
            .unwrap();
        assert_eq!(declared, bytes.len() - (header_end + 4));

        // And it round-trips through the reader.
        let mut cursor = std::io::Cursor::new(bytes);
        let back = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(back["params"]["s"], "héllo — ✓");
    }

    /// End-to-end against the real server. Skipped unless it is installed,
    /// so CI without Node still passes.
    #[test]
    fn live_typescript_diagnostics() {
        let adapter =
            super::super::adapter::adapter_by_name("typescript-language-server").unwrap();
        let super::super::adapter::Source::Npm { entry, .. } = adapter.source else {
            panic!("typescript-language-server should be an npm server");
        };
        let resolved = super::super::node::resolve_npm_server(adapter.name, entry, adapter.args);
        let super::super::node::Resolved::Ready { .. } = resolved else {
            eprintln!("typescript-language-server not installed; skipping live test");
            return;
        };

        let dir = std::env::temp_dir().join(format!("ezicode-lsp-live-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("bad.ts");
        let code = "function f(x: number): string { return x; }\n";
        std::fs::write(&file, code).unwrap();

        let mut mgr = LspManager::new();
        let rx = mgr.event_receiver();
        let client = mgr
            .ensure_server("typescript", Some(&dir))
            .expect("server should start");
        client.did_open(&file, "typescript", code);

        let start = std::time::Instant::now();
        let mut got = false;
        while start.elapsed() < Duration::from_secs(30) {
            if let Ok(LspEvent::Diagnostics { path, diagnostics }) = rx.try_recv() {
                if paths_match(&path, &file) && !diagnostics.is_empty() {
                    got = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert!(got, "expected diagnostics for a deliberate type error");
    }
}
