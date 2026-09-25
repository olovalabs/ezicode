//! Bridges language-server declarations from extensions into ezicode's LSP
//! adapter table.
//!
//! Zed's `[language_servers.<id>]` section names a server and the languages it
//! serves, but the *command* to launch it is produced by the extension's WASM
//! component at runtime. Until the WASM host is fully wired (see
//! [`super::wasm`]), ezicode also honours two optional keys — `command` and
//! `args` — that let an extension declare a native binary (resolved on `$PATH`)
//! to launch directly. This is the pragmatic path that works in the default
//! build today, and it is ignored by Zed (unknown keys), so extensions stay
//! cross-compatible.

use std::sync::OnceLock;

use crate::lsp::adapter::{ServerAdapter, Source};

static ADAPTERS: OnceLock<Vec<ServerAdapter>> = OnceLock::new();

/// Language-server adapters contributed by installed extensions. Built once and
/// memoized; strings are leaked so the adapters satisfy the `&'static` contract
/// the LSP client relies on (there is only a small, bounded number of them).
pub fn extension_adapters() -> &'static [ServerAdapter] {
    ADAPTERS.get_or_init(build)
}

fn build() -> Vec<ServerAdapter> {
    let mut out = Vec::new();
    for ext in super::installed() {
        for (server_id, server) in &ext.manifest.language_servers {
            // Only servers that declare a launchable native binary can be run
            // without the WASM host. Others are surfaced in the UI but not
            // started here.
            let Some(command) = server.command.clone() else {
                continue;
            };
            if command.trim().is_empty() {
                continue;
            }

            let name: &'static str = leak_str(server.name.clone().unwrap_or_else(|| server_id.clone()));
            let binary: &'static str = leak_str(command);
            let args: &'static [&'static str] = leak_args(server.args.clone());

            // Translate the server's declared language *names* into ezicode
            // language *ids*.
            let ids: Vec<String> = server
                .all_languages()
                .into_iter()
                .map(|n| super::languages::language_id_for_name(&n))
                .collect();
            let languages: &'static [&'static str] = leak_langs(ids);

            out.push(ServerAdapter {
                name,
                source: Source::Native { binary },
                args,
                languages,
            });
        }
    }
    out
}

fn leak_str(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn leak_args(v: Vec<String>) -> &'static [&'static str] {
    let leaked: Vec<&'static str> = v.into_iter().map(leak_str).collect();
    Box::leak(leaked.into_boxed_slice())
}

fn leak_langs(v: Vec<String>) -> &'static [&'static str] {
    let leaked: Vec<&'static str> = v
        .into_iter()
        .filter(|s| !s.is_empty())
        .map(leak_str)
        .collect();
    Box::leak(leaked.into_boxed_slice())
}
