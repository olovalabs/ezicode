//! Node runtime + npm package installer — a port of Zed's `NodeRuntime`
//! (`crates/node_runtime/src/node_runtime.rs`).
//!
//! This is the piece that makes language support "just work" with no user
//! setup. Zed never asks you to `npm i -g typescript-language-server`: on the
//! first TypeScript/CSS/HTML/JSON file it opens, it npm-installs the server
//! into a private directory under its data dir and runs it with its own Node.
//!
//! Zed additionally *downloads* a pinned Node build when the system has none.
//! We stop short of that (it means shipping tarball extraction and an updater)
//! and instead use the system `node`, falling back to a clear, actionable
//! status message when it is missing. Everything else follows Zed:
//!
//! * installs live in a per-server container dir — never the user's project,
//!   never a global `npm -g`, so we can't corrupt their toolchain
//!   (Zed: `container_dir`)
//! * a version marker records what we installed, so startup is instant on
//!   every later launch (Zed: `should_install_npm_package`)
//! * installs run on a background thread and are deduplicated, so opening ten
//!   `.ts` files triggers exactly one `npm install`
//!
//! Everything here runs off the UI thread.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Minimum Node version. Zed pins v24 for its managed download and requires
/// >= 22 of a system Node (`SystemNodeRuntime::MIN_VERSION`).
const MIN_NODE_MAJOR: u32 = 18;

/// How long an `npm install` may run before we give up.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(180);

/// Where servers get installed: `<data dir>/language-servers/<server name>`.
///
/// Mirrors Zed's `~/.local/share/zed/languages/<name>` (and the equivalent
/// `%LOCALAPPDATA%\Zed\languages` on Windows).
pub fn language_servers_dir() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
    } else if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
        PathBuf::from(data)
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".local/share")
    } else {
        std::env::temp_dir()
    };
    base.join("ezicode").join("language-servers")
}

/// The container directory for one server (Zed's `container_dir`).
pub fn container_dir(server_name: &str) -> PathBuf {
    language_servers_dir().join(server_name)
}

/// Locate the `node` executable.
pub fn node_binary() -> Option<PathBuf> {
    // An explicit override always wins, so users on unusual setups (nvm,
    // Volta, corporate images) can point us at the right runtime.
    if let Some(explicit) = std::env::var_os("EZICODE_NODE").or_else(|| std::env::var_os("OLOVA_NODE")) {
        let p = PathBuf::from(explicit);
        if p.is_file() {
            return Some(p);
        }
    }
    super::client::find_binary_on_path("node")
}

/// Locate the npm CLI. Prefer the `npm-cli.js` script next to the Node binary
/// and run it *with* Node: on Windows the `npm` shim is a `.cmd` batch file
/// that cannot be spawned directly, which is exactly why Zed keeps a
/// `NPM_PATH` pointing at `node_modules/npm/bin/npm-cli.js`.
fn npm_command() -> Option<Command> {
    let node = node_binary()?;
    if let Some(dir) = node.parent() {
        let candidates = [
            dir.join("node_modules/npm/bin/npm-cli.js"),
            dir.join("../lib/node_modules/npm/bin/npm-cli.js"),
        ];
        for cli in candidates {
            if cli.is_file() {
                let mut cmd = Command::new(&node);
                cmd.arg(cli);
                return Some(cmd);
            }
        }
    }
    // Fall back to whatever `npm` is on PATH.
    let npm = super::client::find_binary_on_path("npm")?;
    if cfg!(windows) {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", &npm.to_string_lossy()]);
        Some(cmd)
    } else {
        Some(Command::new(npm))
    }
}

/// Whether a usable Node is installed, and its version string.
pub fn node_version() -> Option<String> {
    let node = node_binary()?;
    let out = Command::new(node)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    v.strip_prefix('v')
        .and_then(|rest| rest.split('.').next())
        .and_then(|major| major.parse::<u32>().ok())
        .filter(|major| *major >= MIN_NODE_MAJOR)
        .map(|_| v)
}

/// The marker file recording which packages we installed into a container.
fn marker_path(container: &Path) -> PathBuf {
    container.join(".ezicode-installed")
}

/// Zed's `should_install_npm_package`, simplified: install when the entry
/// point is missing, or when the marker doesn't match the requested set.
///
/// Zed compares real semver against the npm registry to auto-upgrade. We
/// deliberately don't: hitting the network on every file open makes startup
/// depend on the registry being reachable. A marker match means "installed
/// and usable", which keeps launches offline-clean and instant.
fn needs_install(container: &Path, entry: &Path, packages: &[String]) -> bool {
    if !entry.exists() {
        return true;
    }
    let want = packages.join(",");
    match std::fs::read_to_string(marker_path(container)) {
        Ok(have) => have.trim() != want,
        Err(_) => true,
    }
}

/// Installs currently in flight, so N concurrent file opens cause one install.
fn in_flight() -> &'static Mutex<HashSet<String>> {
    static IN_FLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Outcome of resolving a server's executable.
#[derive(Debug)]
pub enum Resolved {
    /// Ready to spawn: run `program` with `args`.
    Ready { program: PathBuf, args: Vec<String> },
    /// An install is required first (and has been started by the caller).
    NeedsInstall,
    /// Cannot be resolved, with a user-facing reason.
    Unavailable(String),
}

/// Resolve a Node-based server without installing anything.
///
/// Returns [`Resolved::Ready`] with the *node* binary as the program and the
/// server's JS entry point as the first argument — Zed does exactly this
/// (`LanguageServerBinary { path: node.binary_path(), arguments: [server_path,
/// "--stdio"] }`) so the server always runs under a known-good Node rather
/// than relying on a shebang.
pub fn resolve_npm_server(
    server_name: &str,
    entry: &str,
    args: &[&str],
) -> Resolved {
    let Some(node) = node_binary() else {
        return Resolved::Unavailable(
            "Node.js is not installed — required for TypeScript/JavaScript, CSS, HTML, JSON and YAML support".into(),
        );
    };
    if node_version().is_none() {
        return Resolved::Unavailable(format!(
            "Node.js {MIN_NODE_MAJOR}+ is required for language server support"
        ));
    }

    let container = container_dir(server_name);
    let entry_path = container.join(entry);
    if entry_path.is_file() {
        let mut argv = vec![entry_path.to_string_lossy().into_owned()];
        argv.extend(args.iter().map(|a| a.to_string()));
        return Resolved::Ready {
            program: node,
            args: argv,
        };
    }
    Resolved::NeedsInstall
}

/// Install a server's npm packages into its container directory.
///
/// Blocking — call from a background thread. Returns the entry-point path on
/// success. Mirrors Zed's `npm_install_packages`, including the retry/timeout
/// flags it passes so a flaky registry doesn't hang the editor forever.
pub fn install_npm_server(
    server_name: &str,
    packages: &[String],
    entry: &str,
) -> Result<PathBuf, String> {
    let container = container_dir(server_name);
    let entry_path = container.join(entry);

    if !needs_install(&container, &entry_path, packages) {
        return Ok(entry_path);
    }

    // Deduplicate concurrent installs of the same server.
    {
        let mut guard = in_flight().lock().unwrap();
        if !guard.insert(server_name.to_string()) {
            return Err(format!("{server_name} is already installing"));
        }
    }
    // Ensure the in-flight marker is cleared on every exit path.
    struct Guard(String);
    impl Drop for Guard {
        fn drop(&mut self) {
            in_flight().lock().unwrap().remove(&self.0);
        }
    }
    let _guard = Guard(server_name.to_string());

    std::fs::create_dir_all(&container)
        .map_err(|e| format!("cannot create {}: {e}", container.display()))?;

    // A package.json keeps npm from walking up and installing into a parent
    // directory — the container must stay self-contained.
    let pkg_json = container.join("package.json");
    if !pkg_json.exists() {
        let _ = std::fs::write(
            &pkg_json,
            format!("{{\"name\":\"ezicode-{server_name}\",\"private\":true}}\n"),
        );
    }

    let Some(mut cmd) = npm_command() else {
        return Err("npm was not found alongside Node.js".into());
    };
    cmd.arg("install")
        .args(packages)
        .args([
            "--no-audit",
            "--no-fund",
            "--no-package-lock",
            "--loglevel=error",
            "--fetch-retry-mintimeout",
            "2000",
            "--fetch-retry-maxtimeout",
            "5000",
            "--fetch-timeout",
            "60000",
        ])
        .current_dir(&container)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to run npm install: {e}"))?;

    // Poll for the timeout instead of blocking forever on a wedged install.
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if start.elapsed() > INSTALL_TIMEOUT {
                    let _ = child.kill();
                    return Err(format!("npm install of {server_name} timed out"));
                }
                std::thread::sleep(Duration::from_millis(120));
            }
            Err(e) => return Err(format!("npm install failed: {e}")),
        }
    };

    if !status.success() {
        let mut stderr = String::new();
        if let Some(mut err) = child.stderr.take() {
            use std::io::Read;
            let _ = err.read_to_string(&mut stderr);
        }
        let detail = stderr.lines().last().unwrap_or("unknown error").to_string();
        return Err(format!("npm install of {server_name} failed: {detail}"));
    }

    if !entry_path.is_file() {
        return Err(format!(
            "{server_name} installed but {} is missing",
            entry_path.display()
        ));
    }

    let _ = std::fs::write(marker_path(&container), packages.join(","));
    Ok(entry_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_dirs_are_per_server_and_isolated() {
        let a = container_dir("typescript-language-server");
        let b = container_dir("vscode-css-language-server");
        assert_ne!(a, b);
        // Never install into the user's project or a global prefix.
        assert!(a.ends_with("language-servers/typescript-language-server"));
    }

    #[test]
    fn needs_install_detects_missing_entry_and_marker_drift() {
        let tmp = std::env::temp_dir().join(format!("ezicode-node-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let entry = tmp.join("server.js");
        let pkgs = vec!["a@1".to_string()];

        // No entry point yet.
        assert!(needs_install(&tmp, &entry, &pkgs));

        std::fs::write(&entry, "//").unwrap();
        // Entry exists but nothing recorded.
        assert!(needs_install(&tmp, &entry, &pkgs));

        std::fs::write(marker_path(&tmp), "a@1").unwrap();
        assert!(!needs_install(&tmp, &entry, &pkgs));

        // Requested set changed -> reinstall.
        assert!(needs_install(&tmp, &entry, &["a@2".to_string()]));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
