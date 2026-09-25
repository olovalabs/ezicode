#!/usr/bin/env bash
set -e
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$DIR"

# Ensure cargo is available in PATH
if ! command -v cargo >/dev/null 2>&1 && [ -d "$HOME/.cargo/bin" ]; then
  export PATH="$HOME/.cargo/bin:$PATH"
fi
cmd="${1:-dev}"
case "$cmd" in
  dev) cargo run -p app ;;
  build) cargo build --release -p app ;;
  # Install the desktop entry + hicolor icon theme so the taskbar shows the
  # ezicode logo instead of a generic one. The editor does this for the current
  # user on every launch, so this is only needed if you want to do it without
  # starting the editor (e.g. from a post-install script).
  install) cargo run -p app -- --install-desktop ;;
  *) echo "Usage: ./run.sh [dev|build|install]"; exit 1 ;;
esac
