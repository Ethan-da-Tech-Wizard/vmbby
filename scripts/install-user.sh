#!/usr/bin/env sh
# Program: vmbby - install-user.sh
# Programmer: Ethan-da-Tech-Wizard (written with Claude, an AI assistant)
# Last Modified: 2026-11-18
# Purpose: Installs the built vmbby program, its GUI helper, icon and desktop entry into the current user's home directories.

# Stop at the first error (-e) and treat unset variables as errors (-u).
set -eu

# ROOT is the project folder (the parent of this scripts folder); CDPATH is cleared so cd prints nothing extra.
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
# BIN is the compiled debug program this script installs.
BIN="$ROOT/target/debug/vmbby"

# If the program has not been built yet, tell the user to run cargo build and stop.
if [ ! -x "$BIN" ]; then
  echo "Build first: cargo build" >&2
  exit 1
fi

# Create the folders in the user's home that the files will go into (-p means no error if they exist).
mkdir -p "$HOME/.local/bin"
mkdir -p "$HOME/.local/share/vmbby/assets"
mkdir -p "$HOME/.local/share/applications"
mkdir -p "$HOME/.local/share/icons/hicolor/scalable/apps"

# Copy each file into place. Mode 0755 makes the program runnable; 0644 is plain read/write for the owner.
install -m 0755 "$BIN" "$HOME/.local/bin/vmbby"
install -m 0644 "$ROOT/assets/vmbby_controller.py" "$HOME/.local/share/vmbby/assets/vmbby_controller.py"
install -m 0644 "$ROOT/assets/vmbby-logo.svg" "$HOME/.local/share/icons/hicolor/scalable/apps/vmbby.svg"
install -m 0644 "$ROOT/vmbby.desktop" "$HOME/.local/share/applications/vmbby.desktop"

# Tell the user where it went and what to do next.
echo "Installed vmbby to $HOME/.local/bin/vmbby"
echo "Make sure $HOME/.local/bin is on PATH, then run: vmbby gui"
