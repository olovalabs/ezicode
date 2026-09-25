'use strict';
// These must stay byte-for-byte in sync with the Rust side
// (`app/src/extensions/mod.rs` and `app/src/lsp/node.rs`), otherwise the
// installer writes where the editor never looks.

const os = require('node:os');
const path = require('node:path');

/** `%LOCALAPPDATA%` on Windows, `$XDG_DATA_HOME` or `~/.local/share` elsewhere. */
function dataRoot() {
  if (process.platform === 'win32') {
    return process.env.LOCALAPPDATA || os.tmpdir();
  }
  if (process.env.XDG_DATA_HOME) return process.env.XDG_DATA_HOME;
  if (os.homedir()) return path.join(os.homedir(), '.local', 'share');
  return os.tmpdir();
}

/** Root of everything ezicode installs at runtime. */
function ezicodeDir() {
  return process.env.EZICODE_DATA_DIR || path.join(dataRoot(), 'ezicode');
}

/** Where unpacked extensions live: `<data>/ezicode/extensions/<publisher>.<name>`. */
function extensionsDir() {
  return path.join(ezicodeDir(), 'extensions');
}

/** Converted, ezicode-format theme families: `<data>/ezicode/themes/*.json`. */
function themesDir() {
  return path.join(ezicodeDir(), 'themes');
}

module.exports = { dataRoot, ezicodeDir, extensionsDir, themesDir };
