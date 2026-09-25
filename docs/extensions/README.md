# Extensions in ezicode — and how far VS Code compatibility can go

**Short answer: yes, it is possible — but only in tiers, and one tier is a hard
"no" for a native GPUI editor.** This document explains exactly which VS Code
extensions can run in ezicode, why, what is already implemented in this repo,
and what is left.

---

## 1. The honest answer

A `.vsix` is just a ZIP with a `package.json` in it. Loading it is trivial.
What is *not* trivial is the thing inside: a VS Code extension expects an
object called `vscode` with several thousand members, and expects the editor to
be a browser.

So compatibility splits into three tiers:

| Tier | Extension kind | Examples | ezicode |
| --- | --- | --- | --- |
| **1. Declarative** | Pure data: colour themes, icon themes, snippets, language configs | Dracula, One Dark Pro, Catppuccin, Material Icon Theme | **Works today** — converted at install time, no JavaScript is ever run |
| **2. Headless JS** | Runs code, but its UI is commands, messages, completions, hovers, diagnostics, formatters, language servers | Prettier, ESLint, most `vscode-languageclient` extensions, snippet generators, git helpers | **Works today for the implemented API surface** — runs in a Node extension host, same as VS Code and Theia |
| **3. UI-owning** | Renders its own HTML: webviews, custom editors, tree views, notebooks, debug UIs | Markdown Preview Enhanced, Draw.io, Jupyter, Live Share, Docker panel | **Not supported, and not on the roadmap as-is** — these paint HTML/CSS into a browser view; ezicode paints GPU quads with GPUI. Supporting them means embedding a browser engine, which throws away the reason ezicode exists |

This is the same line Eclipse Theia had to draw, except Theia *is* a browser
app so it can render webviews in an `<iframe>`
([Theia's plugin architecture](https://github.com/eclipse-theia/theia/blob/master/doc/Plugin-API.md)).
Zed, the closest architectural cousin to ezicode, refused tiers 2 and 3
entirely and shipped WebAssembly extensions instead —
["we have no plans to attempt a re-implementation of the VS Code Extension API"](https://github.com/zed-industries/zed/discussions/19158).

**ezicode's bet: tier 1 + tier 2 is ~70% of what people actually install, and
nobody else in the native-editor space offers it.**

---

## 2. What is implemented in this repo right now

```
extensions/                     Node toolchain (no npm dependencies at all)
├── bin/ezicode-ext.js          CLI: search / install / link / list / inspect / pack
├── src/zip.js                  .vsix reader + writer (pure zlib)
├── src/jsonc.js                JSON-with-comments parser (themes need it)
├── src/manifest.js             manifest reader + per-contribution support report
├── src/theme-convert.js        VS Code colour theme  ->  ezicode theme
├── src/install.js              install / uninstall / enable / disable
├── src/openvsx.js              Open VSX registry client
├── src/host/host.js            the extension host process
├── src/host/vscode-api.js      the `vscode` module extensions receive
├── src/host/rpc.js             JSON-RPC, LSP-style Content-Length framing
└── examples/hello-ezicode/     a real VS Code extension used by the tests

app/src/extensions/mod.rs       Rust: discovery of installed extensions
app/src/theme/mod.rs            Rust: loads extension themes next to built-ins
app/src/ui/sidebar/extensions.rs  Rust: the Extensions panel, now real data
```

Run the tests (they cover the zip reader, the theme converter, install/uninstall
and a full extension-host session with a real extension):

```bash
cd extensions && node --test "test/*.test.js"
```

### Install a VS Code theme into ezicode, today

```bash
extensions/bin/ezicode-ext.js install dracula-theme.theme-dracula
# → converts every theme the extension contributes
# → writes ~/.local/share/ezicode/themes/dracula-theme.theme-dracula.json
# → restart ezicode: "Dracula" is in the theme menu and in settings.json
```

The conversion maps VS Code workbench colours onto ezicode's palette, flattens
translucent chrome colours, converts the 16 ANSI terminal colours, and
translates TextMate `tokenColors` (including `fontStyle: italic/bold`) into the
tree-sitter token names ezicode's highlighter uses. Colours the theme leaves
undefined are derived from the editor background so no UI token is ever missing.

### Run a real VS Code extension

```bash
extensions/bin/ezicode-ext.js link extensions/examples/hello-ezicode
extensions/bin/ezicode-ext.js list
extensions/bin/ezicode-ext.js host        # speaks JSON-RPC on stdio
```

`examples/hello-ezicode` is an ordinary VS Code extension — it does
`require('vscode')`, registers commands, a status bar item, a hover provider
and a completion provider. It is not modified for ezicode in any way.

### Check an extension before installing it

```bash
extensions/bin/ezicode-ext.js inspect esbenp.prettier-vscode
```

```
Prettier - Code formatter  esbenp.prettier-vscode@11.0.0
  verdict: partial
  contributes:
    ✓ commands              full      Registered in the command palette…
    ✓ configuration         full      Merged into settings.json…
    ~ languages             partial   File associations and comment/bracket config…
    ✗ jsonValidation        planned
```

---

## 3. Architecture

```
┌──────────────────────────────┐        ┌─────────────────────────────┐
│ ezicode (Rust, GPUI)         │        │ extension host (Node)       │
│                              │        │                             │
│  workspace / editor / UI     │        │  require('vscode') shim     │
│  theme registry ◀────────────┼── json │  activation events          │
│  command palette             │  rpc   │  command registry           │
│  completion + hover ◀────────┼───────▶│  providers                  │
│  Extensions panel            │ stdio  │  ┌───────────────────────┐  │
└──────────────────────────────┘        │  │ extension A  B  C …   │  │
        ▲                                │  └───────────────────────┘  │
        │ reads at startup               └─────────────────────────────┘
  <data>/ezicode/themes/*.json                     one process,
  <data>/ezicode/extensions/<id>/                  crash-isolated
```

Key decisions:

1. **Extensions never run in the editor process.** A hung extension cannot
   freeze a GPUI frame. This is exactly why VS Code has an "extension host"
   too.
2. **The transport is JSON-RPC with LSP-style `Content-Length` framing** —
   ezicode already speaks this in `app/src/lsp/client.rs`, so the Rust side
   reuses its reader/writer instead of inventing a second protocol.
3. **Declarative work happens at install time, not at runtime.** A theme is
   converted once by `ezicode-ext` and cached as an ezicode theme file. The
   editor never parses TextMate at startup, so the "install 30 themes" case
   costs nothing.
4. **Node is already a dependency.** ezicode provisions Node-based language
   servers (`app/src/lsp/node.rs`), so the extension host adds no new runtime
   requirement.

### RPC surface (implemented)

| Direction | Method | Purpose |
| --- | --- | --- |
| editor → host | `initialize` | extension list, contributed commands, API version |
| editor → host | `activate` | activate by id or by activation event |
| editor → host | `commands/list`, `commands/execute` | command palette |
| editor → host | `document/didOpen\|didChange\|didSave\|didClose` | document sync |
| editor → host | `editor/didChangeActive` | `window.activeTextEditor` |
| editor → host | `provider/hover`, `provider/completion`, `provider/format` | language features |
| editor → host | `configuration/didChange`, `shutdown` | settings + lifecycle |
| host → editor | `window/showMessage`, `window/showQuickPick`, `window/showInputBox` | UI requests |
| host → editor | `window/statusBarItem`, `window/setStatusBarMessage`, `window/outputChannel` | chrome |
| host → editor | `workspace/applyEdit`, `editor/revealRange` | buffer edits |
| host → editor | `languages/diagnostics` | squiggles |
| host → editor | `commands/registered`, `extension/activated`, `log` | bookkeeping |

### `vscode` API implemented by the shim

`commands` · `window` (messages, quick pick, input box, status bar, output
channel, active editor, edits) · `workspace` (folders, configuration,
documents, events, `fs`) · `languages` (hover, completion, formatting, code
actions, diagnostics) · `env` · `extensions` · plus the types extensions
construct directly: `Uri`, `Position`, `Range`, `Selection`, `Disposable`,
`EventEmitter`, `MarkdownString`, `Hover`, `CompletionItem`, `TextEdit`,
`WorkspaceEdit`, `Diagnostic`, `ThemeColor`, `StatusBarAlignment`, …

Not implemented yet: `debug`, `tasks`, `notebooks`, `scm`, `tests`,
`authentication`, `window.createWebviewPanel`, `window.createTreeView`.

---

## 4. Which registry — and the legal part

**ezicode must not use the Microsoft Visual Studio Marketplace.** Its Terms of
Use restrict marketplace offerings to "In-Scope Products and Services" —
Microsoft's own Visual Studio family. That is why VSCodium, Gitpod, Theia,
Cursor and Windsurf all default to
[Open VSX](https://open-vsx.org), the Eclipse Foundation registry created for
exactly this case ([background](https://devclass.com/2023/06/27/open-vsx-alternative-to-vs-code-marketplace-saved-from-closure-by-new-eclipse-working-group/)).

Open VSX serves direct `.vsix` downloads over a documented API and hosts
10 000+ extensions. `extensions/src/openvsx.js` talks to it; the registry is
overridable with `EZICODE_REGISTRY` for self-hosted mirrors.

Two consequences to communicate to users honestly:

- Some extensions (Microsoft's C#, Pylance, Remote-*) are **not** on Open VSX
  and cannot legally be shipped by ezicode.
- Open VSX has no mandatory review. Extensions run as normal Node processes
  with the user's privileges — the same trust model as VS Code. The
  `enabled: false` switch, per-extension process isolation and the
  install-time report are the current mitigations; a permissions manifest is
  on the roadmap.

---

## 5. Roadmap

| Phase | Scope | State |
| --- | --- | --- |
| **0** | `.vsix` reader, Open VSX client, install/uninstall/enable, compatibility report | **done** |
| **1** | Colour themes converted and loaded by the editor; Extensions panel backed by real data | **done** |
| **2** | Extension host process + `vscode` shim: commands, messages, status bar, hover, completion, formatting, diagnostics | **done (host side); Rust side needs wiring**: spawn the host from `Workspace`, feed document sync, merge host commands into the command palette, merge host completions/hovers into the existing LSP popovers |
| **3** | Icon themes, snippets, `contributes.languages` (comments/brackets), keybindings, `configuration` schema in the settings UI | next |
| **4** | `vscode-languageclient` support, so entire language extensions work unmodified (this is where the big ecosystem win is) | after 3 |
| **5** | Sandboxing (permissions in the manifest), extension marketplace UI inside the editor, background auto-update | later |
| **—** | Webviews, custom editors, tree views, notebooks, debug adapters UI | rejected while ezicode is a pure-GPUI renderer; revisit only if a native HTML surface is ever added |

### Phase 2 wiring checklist (the next PR)

1. `app/src/extensions/host.rs` — spawn
   `node extensions/src/host/host.js --extensions-dir <dir>`, reuse the
   `Content-Length` codec from `lsp::client`.
2. Send `initialize` with the workspace folders and `settings.json`, then
   `activate { event: "onStartupFinished" }`.
3. Forward buffer open/change/save from `Workspace` as `document/*`
   notifications.
4. Merge `commands` from `initialize` into the command palette; run the
   selected one through `commands/execute`.
5. Handle `window/showMessage` with the existing notification UI and
   `workspace/applyEdit` through the editor's rope buffer.
6. Ask `provider/hover` and `provider/completion` alongside the LSP client and
   merge results.

---

## 6. Writing an ezicode extension

Write a VS Code extension. That is the whole point — there is no separate
ezicode extension format.

```bash
mkdir my-extension && cd my-extension
cat > package.json <<'JSON'
{
  "name": "my-extension",
  "publisher": "me",
  "version": "0.0.1",
  "engines": { "vscode": "^1.85.0" },
  "main": "./extension.js",
  "activationEvents": ["onStartupFinished"],
  "contributes": { "commands": [{ "command": "my.hello", "title": "My: Hello" }] }
}
JSON
cat > extension.js <<'JS'
const vscode = require('vscode');
exports.activate = (context) => {
  context.subscriptions.push(
    vscode.commands.registerCommand('my.hello', () =>
      vscode.window.showInformationMessage('hello from my extension')
    )
  );
};
JS

ezicode-ext link .      # install it into ezicode
ezicode-ext pack .      # or build a .vsix to publish on Open VSX
```

The same folder installs into VS Code with `code --install-extension`, so
authors are never writing "ezicode-only" code.
