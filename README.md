<div align="center">

<img src="assets/logo/ezicode.png" alt="ezicode Logo" width="140" height="140" />

# ezicode

**A blazingly fast, GPU-accelerated native code editor built with Rust, GPUI, and Tree-sitter.**

[![Rust](https://img.shields.io/badge/rust-2021%20edition-orange.svg?style=flat-square&logo=rust)](https://www.rust-lang.org)
[![GPUI](https://img.shields.io/badge/UI%20Framework-GPUI-blueviolet.svg?style=flat-square)](https://github.com/zed-industries/zed)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-informational.svg?style=flat-square)](https://github.com)
[![License](https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-blue.svg?style=flat-square)](LICENSE)

*Combining the raw speed and hardware-accelerated rendering of Zed with the friendly ergonomics and workflow of modern code editors.*

[Quick Start](#-quick-start) •
[Features](#-key-features) •
[Language Servers](#-language-servers--lsp) •
[Keyboard Shortcuts](#-keyboard-shortcuts) •
[Configuration](#-configuration)

</div>

---

## ⚡ Key Features

### 🚀 Hardware-Accelerated Editor Core
- **GPUI Rendering**: Butter-smooth 60+ FPS rendering driven directly by your GPU.
- **Instant Cold Starts**: Starts up in milliseconds with near-zero idle resource consumption.
- **Rope Buffer Architecture**: Effortlessly view and edit massive source files without UI freezes.
- **Tree-sitter Syntax Highlighting**: Accurate, semantic, AST-based incremental highlighting.

### 🖥️ Integrated GPU Terminal (`gpui-terminal`)
- **Alacritty + PTY Engine**: Hardware-accelerated terminal emulation with full 24-bit RGB truecolor.
- **Rich TUI Support**: Seamlessly run interactive console apps like `vim`, `nano`, `htop`, `lazygit`, and AI agents.
- **Multi-Tab Sessions**: Launch and switch between multiple shell instances with process lifecycle tracking (running, exited, error status dots).
- **Auto-Detected Shells**: Automatically finds Git Bash, PowerShell, or `pwsh` on Windows, and `zsh`, `fish`, or `bash` on Unix systems.
- **Zed-Style Navigation**: Dedicated shortcuts for splitting, cycling, quick-switching (`Alt+1..5`), and maximizing terminal views.

### 🔌 Zero-Config Language Server Protocol (LSP)
- **Automatic Server Provisioning**: Node-based language servers install themselves silently on demand into a private sandboxed directory (`~/.local/share/ezicode/language-servers/`)—no global `npm install` required.
- **Toolchain Discovery**: Auto-detects local system compilers and language servers installed on `PATH` (e.g., `rust-analyzer`, `gopls`, `clangd`, `basedpyright`).
- **Rich LSP Features**: Real-time diagnostics (errors & warnings), code completion popovers, hover tooltips, go-to-definition (`F12`), and document formatting (`Shift+Alt+F`).

### 🐙 Built-In Git & Interactive Diff Viewer
- **Source Control Sidebar**: Fast workspace status detection (`git status --porcelain=v1 -z`) listing staged, unstaged, and untracked changes.
- **One-Click Staging**: Stage, unstage, discard, and commit changes directly from the UI.
- **Side-by-Side & Unified Diffs**: Full-featured split or unified diff editor with character-level intra-line highlights, synced scrolling, and additions/deletions statistics.

### 📁 High-Performance Project Explorer
- **Virtual Viewport (`uniform_list`)**: Virtualized rendering that scales to repositories with hundreds of thousands of files.
- **Lazy Directory Loading**: Only reads directories on expansion; caches directory snapshots to avoid redundant disk I/O.
- **Live Filesystem Watcher**: Instant UI updates when files are changed, created, or deleted externally (`notify`).
- **Complete File Operations**: Inline creation of files and folders, inline rename (`F2`), safe drag-and-drop file moving with loop protection, copy/cut/paste, and auto-reveal for active files.

### 🎨 Zed-Compatible Themes & Typography
- **Exact Zed JSON Themes**: Shipped with **GitHub Dark**, **Ayu** (Dark, Mirage, Light), and **Gruvbox** families, covering both UI chrome tokens and 46 Tree-sitter syntax tokens.
- **Curated Typography**: Pre-bundled with **IBM Plex Sans** (for UI) and **Lilex** (for the code buffer and terminal), automatically registered with GPUI at launch.
- **Rich File Icons**: Zed's complete SVG icon theme mapping file extensions to high-fidelity icons.

---

## 🚀 Quick Start

### Prerequisites
- **Rust toolchain** (stable 1.80+ recommended): [rustup.rs](https://rustup.rs/)
- **Git** (for source control features)
- **Node.js & npm** (optional, enables automatic installation of web/JS language servers)

### Running from Source

```bash
# Clone the repository
git clone https://github.com/olovalabs/ezicode.git
cd ezicode

# Run in development mode
cargo run -p app

# Or use the convenience scripts
# On Linux / macOS:
./run.sh dev

# On Windows:
run.cmd dev
```

### Building for Release

```bash
# Compile optimized release binary with LTO
cargo build --release -p app

# The binary will be generated at target/release/app (or app.exe on Windows)
```

---

## 🔌 Language Servers (LSP)

`ezicode` features an automated language server manager modeled after Zed's architecture:

| Language Server | Handled Languages | Installation Mode |
|---|---|---|
| `typescript-language-server` | TypeScript (`.ts`, `.tsx`), JavaScript (`.js`, `.jsx`) | **Auto-installed** on first use via Node |
| `vscode-css-language-server` | CSS, SCSS, SASS | **Auto-installed** on first use via Node |
| `vscode-html-language-server` | HTML, HTM, XHTML | **Auto-installed** on first use via Node |
| `json-language-server` | JSON, JSONC | **Auto-installed** on first use via Node |
| `yaml-language-server` | YAML, YML | **Auto-installed** on first use via Node |
| `bash-language-server` | Shell script (`.sh`, `.bash`, `.zsh`) | **Auto-installed** on first use via Node |
| `dockerfile-language-server` | Dockerfile, Containerfile | **Auto-installed** on first use via Node |
| `rust-analyzer` | Rust (`.rs`) | Auto-discovered on `PATH` |
| `gopls` | Go (`.go`) | Auto-discovered on `PATH` |
| `basedpyright` / `pyright` | Python (`.py`, `.pyw`) | Auto-discovered on `PATH` |
| `clangd` | C / C++ (`.c`, `.cpp`, `.h`, `.hpp`) | Auto-discovered on `PATH` |
| `zls` | Zig (`.zig`) | Auto-discovered on `PATH` |
| `lua-language-server` | Lua (`.lua`) | Auto-discovered on `PATH` |
| `taplo` | TOML (`.toml`) | Auto-discovered on `PATH` |
| `intelephense` | PHP (`.php`) | Auto-discovered on `PATH` |
| `ruby-lsp` | Ruby (`.rb`) | Auto-discovered on `PATH` |

> [!NOTE]
> When opening a file whose server is managed via Node (like TypeScript or CSS), `ezicode` installs the server into a private, isolated runtime directory. The status bar indicates `◌ installing…`, and once complete, diagnostics and completions attach automatically without restarting.

---

## ⌨️ Keyboard Shortcuts

### General & Workspace
| Shortcut | Action |
|---|---|
| <kbd>Ctrl</kbd> + <kbd>N</kbd> | New Untitled File |
| <kbd>Ctrl</kbd> + <kbd>O</kbd> | Open File |
| <kbd>Ctrl</kbd> + <kbd>S</kbd> | Save Current Buffer |
| <kbd>Ctrl</kbd> + <kbd>,</kbd> | Open Settings |
| <kbd>Ctrl</kbd> + <kbd>B</kbd> | Toggle Sidebar |
| <kbd>Ctrl</kbd> + <kbd>Shift</kbd> + <kbd>E</kbd> | Focus Explorer |
| <kbd>Ctrl</kbd> + <kbd>Shift</kbd> + <kbd>F</kbd> | Focus Search |
| <kbd>Ctrl</kbd> + <kbd>Shift</kbd> + <kbd>G</kbd> | Focus Source Control |
| <kbd>Ctrl</kbd> + <kbd>Shift</kbd> + <kbd>X</kbd> | Focus Extensions |

### Editor & Navigation
| Shortcut | Action |
|---|---|
| <kbd>Ctrl</kbd> + <kbd>W</kbd> | Close Active Tab |
| <kbd>Ctrl</kbd> + <kbd>Tab</kbd> | Next Tab |
| <kbd>Ctrl</kbd> + <kbd>Shift</kbd> + <kbd>Tab</kbd> | Previous Tab |
| <kbd>F12</kbd> | Go to Definition |
| <kbd>Shift</kbd> + <kbd>Alt</kbd> + <kbd>F</kbd> | Format Document (LSP) |
| <kbd>Ctrl</kbd> + <kbd>F</kbd> | Find in Buffer |
| <kbd>Ctrl</kbd> + <kbd>=</kbd> / <kbd>+</kbd> | Increase Editor Font Size |
| <kbd>Ctrl</kbd> + <kbd>-</kbd> | Decrease Editor Font Size |
| <kbd>Ctrl</kbd> + <kbd>0</kbd> | Reset Font Size |

### Integrated Terminal
| Shortcut | Action |
|---|---|
| <kbd>Ctrl</kbd> + <kbd>`</kbd> / <kbd>Ctrl</kbd> + <kbd>J</kbd> | Toggle Terminal Panel |
| <kbd>Ctrl</kbd> + <kbd>Shift</kbd> + <kbd>`</kbd> | New Terminal Session |
| <kbd>Ctrl</kbd> + <kbd>Shift</kbd> + <kbd>W</kbd> | Close Active Terminal |
| <kbd>Ctrl</kbd> + <kbd>Shift</kbd> + <kbd>K</kbd> | Clear Terminal Buffer |
| <kbd>Alt</kbd> + <kbd>1</kbd> .. <kbd>5</kbd> | Jump to Terminal Tab 1 – 5 |
| <kbd>Alt</kbd> + <kbd>→</kbd> / <kbd>←</kbd> | Next / Previous Terminal Tab |

### File Explorer
| Shortcut | Action |
|---|---|
| <kbd>↑</kbd> / <kbd>↓</kbd> | Navigate File Tree Rows |
| <kbd>→</kbd> / <kbd>←</kbd> | Expand / Collapse Folder |
| <kbd>Enter</kbd> | Open File / Toggle Folder |
| <kbd>F2</kbd> | Rename File or Folder |
| <kbd>Delete</kbd> | Delete File or Folder |
| <kbd>Ctrl</kbd> + <kbd>C</kbd> / <kbd>X</kbd> / <kbd>V</kbd> | Copy / Cut / Paste File |

---

## ⚙️ Configuration

`ezicode` stores its configuration in a standard JSON format located at:

- **Windows**: `%APPDATA%\ezicode\settings.json`
- **macOS**: `~/Library/Application Support/ezicode/settings.json`
- **Linux**: `~/.config/ezicode/settings.json`

### Example `settings.json`
```json
{
  "editor.fontSize": 14.5,
  "editor.fontFamily": "Lilex",
  "editor.tabSize": 4,
  "editor.autoSave": "afterDelay",
  "editor.autoSaveDelay": 1000,
  "workbench.theme": "GitHub Dark",
  "terminal.fontSize": 13.5
}
```

### Configurable Options
- `editor.fontSize`: Font size in pixels for the code buffer (default: `14.5`).
- `editor.fontFamily`: Custom monospace font family (falls back to bundled `Lilex`).
- `editor.tabSize`: Number of spaces per tab indent (default: `4`).
- `editor.autoSave`: Auto-save behavior: `"off"`, `"afterDelay"`, or `"onFocusChange"`.
- `editor.autoSaveDelay`: Debounce duration in milliseconds when `"afterDelay"` is chosen (default: `1000`).
- `workbench.theme`: Active color theme (e.g. `"GitHub Dark"`, `"Ayu Dark"`, `"Gruvbox Dark"`).

---

## 📂 Project Architecture

```text
ezicode/
├── app/                      # Main application crate
│   ├── assets/               # Embedded fonts, themes, file icons & logos
│   │   ├── file_icons/       # Zed SVG file icons
│   │   ├── fonts/            # IBM Plex Sans & Lilex TTF font binaries
│   │   ├── logo/             # ezicode branding & application icons
│   │   ├── themes/           # Zed JSON theme definitions
│   │   └── ui_icons/         # Activity bar & panel glyphs
│   └── src/
│       ├── actions.rs        # GPUI actions and command declarations
│       ├── fs_tree.rs        # Virtualized file tree model & filesystem ops
│       ├── git.rs            # Native Git porcelain parser and process runner
│       ├── lang.rs           # Language detection & LSP server mapping
│       ├── lsp/              # LSP JSON-RPC client, Node auto-installer & adapters
│       ├── terminal/         # Integrated GPU terminal panel & process lifecycle
│       ├── theme/            # Zed JSON theme loader & color token extraction
│       ├── ui/               # UI components: tabs, status bar, diffs, settings
│       └── workspace/        # Main workspace entity, state management & render loop
├── components/               # Vendored and optimized GPUI components
├── gpui-terminal/            # Alacritty terminal emulator bindings for GPUI
├── run.cmd                   # Windows developer helper script
├── run.sh                    # Unix developer helper script
└── Cargo.toml                # Root workspace configuration & compiler profiles
```

---

## 🛠️ Diagnostics & Troubleshooting

- **Panic Logger**: On unexpected crashes, `ezicode` automatically generates detailed trace logs in your system temporary directory (`ezicode-panic.log`) instead of silently exiting.
- **Node LSP Debugging**: If language servers fail to download or start, verify that Node.js is accessible on your system PATH (`node -v`). Downloaded language servers are located in `~/.local/share/ezicode/language-servers`.

---

<div align="center">

Made with ❤️ by the **olovalabs** team.

</div>
