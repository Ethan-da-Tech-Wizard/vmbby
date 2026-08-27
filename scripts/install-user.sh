#!/usr/bin/env sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
BIN="$ROOT/target/debug/vmbby"

if [ ! -x "$BIN" ]; then
  echo "Build first: cargo build" >&2
  exit 1
fi

mkdir -p "$HOME/.local/bin"
mkdir -p "$HOME/.local/share/vmbby/assets"
mkdir -p "$HOME/.local/share/applications"
mkdir -p "$HOME/.local/share/icons/hicolor/scalable/apps"

install -m 0755 "$BIN" "$HOME/.local/bin/vmbby"
install -m 0644 "$ROOT/assets/vmbby_controller.py" "$HOME/.local/share/vmbby/assets/vmbby_controller.py"
install -m 0644 "$ROOT/assets/vmbby-logo.svg" "$HOME/.local/share/icons/hicolor/scalable/apps/vmbby.svg"
install -m 0644 "$ROOT/vmbby.desktop" "$HOME/.local/share/applications/vmbby.desktop"

echo "Installed vmbby to $HOME/.local/bin/vmbby"
echo "Make sure $HOME/.local/bin is on PATH, then run: vmbby gui"
