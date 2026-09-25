# `ezicode-ext` — extension toolchain

Zero-dependency Node tooling that gives ezicode VS Code-compatible extensions:
a `.vsix` reader/packer, a VS Code → ezicode theme converter, an Open VSX
client, and the extension host process that actually runs extension code.

See [`docs/extensions/README.md`](../docs/extensions/README.md) for the
architecture and for what is and is not supported.

## Requirements

Node 18+. Nothing else — there are no npm dependencies, on purpose: the
toolchain ships with the editor and must not drag a `node_modules` tree behind
it.

## Commands

```bash
ezicode-ext search dracula                     # search Open VSX
ezicode-ext install dracula-theme.theme-dracula  # install from Open VSX
ezicode-ext install ./some-extension.vsix      # …or from a local file
ezicode-ext link ./my-extension                # install an unpacked folder (dev loop)
ezicode-ext list                               # what is installed
ezicode-ext inspect esbenp.prettier-vscode     # what ezicode supports in it
ezicode-ext disable <id> / enable <id>
ezicode-ext uninstall <id>
ezicode-ext import-theme ./my-color-theme.json # convert a bare theme file
ezicode-ext pack ./my-extension -o out.vsix    # build a .vsix (no vsce needed)
ezicode-ext host                               # run the extension host on stdio
```

Run it straight from the repo while developing:

```bash
node extensions/bin/ezicode-ext.js list
```

## Where things land

| Path | Contents |
| --- | --- |
| `<data>/ezicode/extensions/<publisher>.<name>/` | unpacked extension + `.ezicode.json` install metadata |
| `<data>/ezicode/themes/<publisher>.<name>.json` | themes converted to ezicode's format |
| `<data>/ezicode/extensions/.state/<id>/` | `globalState` / `workspaceState` for the extension |

`<data>` is `%LOCALAPPDATA%` on Windows and `$XDG_DATA_HOME` (or
`~/.local/share`) elsewhere — the same convention
`app/src/lsp/node.rs` uses for language servers. Override the whole root with
`EZICODE_DATA_DIR`, and the registry with `EZICODE_REGISTRY`.

## Tests

```bash
cd extensions
node --test "test/*.test.js"
```

`test/host.test.js` spawns the real host, loads the unmodified VS Code
extension in `examples/hello-ezicode`, and drives it over JSON-RPC: activation,
a command that reads `workspace.getConfiguration`, a status bar item, a hover
provider, a completion provider and an `editor.edit()` round trip.

## Talking to the host by hand

```bash
node src/host/host.js --extensions-dir ~/.local/share/ezicode/extensions
```

Then write LSP-framed JSON-RPC to stdin:

```
Content-Length: 61\r\n\r\n{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
```
