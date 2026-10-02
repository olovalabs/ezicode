<div align="center">

<img width="800" alt="ezicode" src="https://github.com/user-attachments/assets/2020bb4b-0cd8-484d-98c5-e32721be6053" />

# ezicode

**GPU-accelerated, lightweight native code editor built with Rust and GPUI.**

[![Language](https://img.shields.io/badge/Language-Rust-orange?style=flat-square)](https://www.rust-lang.org)
[![Framework](https://img.shields.io/badge/Framework-GPUI-blueviolet?style=flat-square)](https://github.com/zed-industries/zed)
[![Platform](https://img.shields.io/badge/Platform-Windows%20%7C%20macOS%20%7C%20Linux-informational?style=flat-square)](https://github.com/olovalabs/ezicode)
[![License](https://img.shields.io/badge/License-MIT%20%2F%20Apache--2.0-blue?style=flat-square)](LICENSE)

</div>

---

> **ezicode** is a fast native desktop code editor engineered for responsiveness, low latency, and minimal resource usage. It combines direct GPU rendering with Tree-sitter semantic parsing, an integrated terminal, and zero-configuration language tooling.

---

### About

ezicode replaces heavy web-based editor architectures with a lean, compiled native binary. Built using Rust and GPUI, it offers instant cold starts, high-frame-rate rendering, and low memory consumption while providing full modern editor workflows.

---

### Core Architecture

| Component | Engine | Description |
| :--- | :--- | :--- |
| **Rendering** | GPUI | Direct GPU acceleration across Vulkan, DirectX, Metal, and Wayland/X11 |
| **Buffer Store** | Rope | Non-blocking, low-memory handling for large files without UI freezes |
| **Highlighter** | Tree-sitter | Fast, incremental AST-based syntax highlighting |
| **Terminal** | Alacritty / PTY | GPU-accelerated terminal with 24-bit truecolor and multi-tab sessions |
| **Intellisense** | LSP JSON-RPC | Automatic language server provisioning, diagnostics, and code formatting |
| **Theme Engine** | Zed JSON | Native compatibility with Zed themes and typography tokens |

---

### Key Capabilities

* **Hardware-Accelerated UI**: 60+ FPS butter-smooth rendering driven directly by the GPU.
* **Instant Startups**: Millisecond launch times with near-zero idle resource footprint.
* **Zero-Config LSP**: On-demand sandboxed language server installation and automatic toolchain discovery on `PATH`.
* **Integrated Terminal Dock**: Embedded bottom and side terminal panels with independent shell sessions.
* **Native Git Integration**: Built-in status tracking, side-by-side and unified diff viewers, and stage management.
* **Sticky Scroll Explorer**: Virtualized directory tree with sticky parent folder headers for deep navigation.

---

### Quick Start

```bash
# Clone the repository
git clone https://github.com/olovalabs/ezicode.git
cd ezicode

# Run in development mode
cargo run -p ezicode

# Build release binary
cargo build --release -p ezicode
```

---

<div align="center">

<sub>Maintained by **olovalabs**. Licensed under MIT and Apache-2.0.</sub>

</div>
