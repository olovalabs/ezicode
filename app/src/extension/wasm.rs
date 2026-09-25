//! Experimental host for running compiled extension logic (`extension.wasm`).
//!
//! Only compiled with `--features wasm-extensions`, so the default build never
//! depends on `wasmtime`.
//!
//! # What works here
//!
//! * A `wasmtime` engine with the **component model** enabled.
//! * A component `Linker` wired with **WASI** ([`wasmtime_wasi`]), which is what
//!   every extension built with the `zed_extension_api` component adapter needs
//!   for its `wasi:*` imports.
//! * Each extension's `extension.wasm` is loaded, validated, and an attempt is
//!   made to instantiate it. Components that only require WASI instantiate
//!   successfully; components that also import the `zed:extension/*` host
//!   interfaces fail to instantiate with a clear "unknown import" error until
//!   those host functions are implemented.
//!
//! # What remains
//!
//! Implementing the `zed:extension` world's host imports (github, http-client,
//! nodejs, platform, process, worktree, …) and calling the guest exports
//! (`language-server-command`, etc.). The complete, matching WIT is vendored at
//! [`app/wit/since_v0.6.0`](../../../wit/since_v0.6.0) and is ready to drive
//! `wasmtime::component::bindgen!`. Because Zed's own host is tightly coupled to
//! Zed-internal crates, that binding layer is deliberately kept as a focused
//! follow-up rather than a blind port.
//!
//! In the meantime, language servers that ship as a normal binary can be run
//! today via the native path — see [`super::lsp`].

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::{IoView, ResourceTable, WasiCtx, WasiCtxBuilder, WasiView};

use super::InstalledExtension;

/// Per-extension store state: a WASI context plus its resource table.
struct Ctx {
    table: ResourceTable,
    wasi: WasiCtx,
}

impl IoView for Ctx {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

impl WasiView for Ctx {
    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.wasi
    }
}

/// A single extension's WASM component, loaded and validated.
pub struct LoadedComponent {
    pub id: String,
    pub component: Component,
    /// Whether the component could be instantiated against a WASI-only linker.
    pub instantiated: bool,
}

struct Host {
    #[allow(dead_code)]
    engine: Engine,
    components: HashMap<String, LoadedComponent>,
}

static HOST: OnceLock<Mutex<Host>> = OnceLock::new();

fn build_engine() -> Engine {
    let mut config = Config::new();
    config.wasm_component_model(true);
    // Synchronous instantiation keeps the host simple; extensions that need to
    // block (e.g. a long-running language server) are launched as OS processes,
    // not on the WASM thread.
    config.async_support(false);
    Engine::new(&config).expect("failed to create wasmtime engine")
}

fn new_store(engine: &Engine) -> Store<Ctx> {
    let wasi = WasiCtxBuilder::new().inherit_stdio().build();
    Store::new(
        engine,
        Ctx {
            table: ResourceTable::new(),
            wasi,
        },
    )
}

/// Load every `extension.wasm` found among the installed extensions and attempt
/// to instantiate each against a WASI-enabled linker.
pub fn init(extensions: &[InstalledExtension]) {
    let engine = build_engine();

    let mut linker: Linker<Ctx> = Linker::new(&engine);
    if let Err(e) = wasmtime_wasi::add_to_linker_sync(&mut linker) {
        eprintln!("wasm-extensions: failed to set up WASI linker: {e}");
        return;
    }

    let mut components = HashMap::new();
    for ext in extensions {
        let path = ext.wasm_path();
        if !path.is_file() {
            continue;
        }
        let id = ext.manifest.effective_id();
        let component = match Component::from_file(&engine, &path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("wasm-extensions: failed to load {}: {e}", path.display());
                continue;
            }
        };

        // Try to instantiate to see how far the component gets against a
        // WASI-only linker. This surfaces which extensions need the full
        // `zed:extension` host imports.
        let mut store = new_store(&engine);
        let instantiated = match linker.instantiate(&mut store, &component) {
            Ok(_) => {
                println!("wasm-extensions: instantiated `{id}` (WASI-only)");
                true
            }
            Err(e) => {
                println!(
                    "wasm-extensions: `{id}` loaded but needs host imports not yet provided: {e}"
                );
                false
            }
        };

        components.insert(
            id.clone(),
            LoadedComponent {
                id,
                component,
                instantiated,
            },
        );
    }

    let _ = HOST.set(Mutex::new(Host { engine, components }));
}

/// Number of successfully loaded WASM components (for diagnostics/UI).
#[allow(dead_code)]
pub fn loaded_count() -> usize {
    HOST.get()
        .and_then(|h| h.lock().ok().map(|g| g.components.len()))
        .unwrap_or(0)
}

/// Number of components that instantiated against the WASI-only linker.
#[allow(dead_code)]
pub fn instantiated_count() -> usize {
    HOST.get()
        .and_then(|h| {
            h.lock()
                .ok()
                .map(|g| g.components.values().filter(|c| c.instantiated).count())
        })
        .unwrap_or(0)
}
