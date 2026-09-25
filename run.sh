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
  *) echo "Usage: ./run.sh [dev|build]"; exit 1 ;;
esac
