#!/usr/bin/env sh

# ==============================================================================
# Program: install-user.sh
# Programmer: Ethan-da-Tech-Wizard
# Last Modified: November 19, 2026
# Purpose: User-level installation and deployment orchestrator for VMBby.
#          Validates release/debug build artifacts, establishes local XDG Directory
#          Standard hierarchies (~/.local/bin, ~/.local/share), and atomically deploys
#          the compiled hypervisor CLI engine, GTK4/Libadwaita control panel,
#          scalable SVG icon assets, and desktop application launcher entries.
#          Ensures idempotent file mode permissions (POSIX 0755 / 0644) and safe execution.
# ==============================================================================

# Halt script execution immediately if any command returns a non-zero exit code (-e)
# or if any uninitialized variable is evaluated (-u), preventing partial deployments.
set -eu

# Resolve the absolute canonical root path of the repository.
# CDPATH is cleared to prevent cd output pollution in environments with custom search paths.
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
BIN="$ROOT/target/debug/vmbby"

# ------------------------------------------------------------------------------
# Pre-Flight Build Validation: Verify binary artifact compilation
# ------------------------------------------------------------------------------
# Ensure the Cargo debug executable exists and possesses execution bits before proceeding.
if [ ! -x "$BIN" ]; then
  echo "Build artifact not found or not executable. Compile first: cargo build" >&2
  exit 1
fi

# ------------------------------------------------------------------------------
# Directory Hierarchy Setup (XDG Base Directory Specification conformance)
# ------------------------------------------------------------------------------
# ~/.local/bin: User-level executable path, standard for user-space system binaries
# ~/.local/share/vmbby/assets: Packaged runtime assets for the Python/Libadwaita portal
# ~/.local/share/applications: Registered desktop menu entries parsed by GNOME/KDE/Sway/Hyprland
# ~/.local/share/icons/hicolor/scalable/apps: Freedesktop icon theme path for scalable SVG logos
mkdir -p "$HOME/.local/bin"
mkdir -p "$HOME/.local/share/vmbby/assets"
mkdir -p "$HOME/.local/share/applications"
mkdir -p "$HOME/.local/share/icons/hicolor/scalable/apps"

# ------------------------------------------------------------------------------
# File Installation and Permission Hardening
# ------------------------------------------------------------------------------
# Install hypervisor CLI binary with rwxr-xr-x (0755) permissions
install -m 0755 "$BIN" "$HOME/.local/bin/vmbby"

# Install Python GTK4 controller script with rw-r--r-- (0644) non-executable permissions
install -m 0644 "$ROOT/assets/vmbby_controller.py" "$HOME/.local/share/vmbby/assets/vmbby_controller.py"

# Install vector graphics branding asset
install -m 0644 "$ROOT/assets/vmbby-logo.svg" "$HOME/.local/share/icons/hicolor/scalable/apps/vmbby.svg"

# Install desktop launcher entry for system application menus
install -m 0644 "$ROOT/vmbby.desktop" "$HOME/.local/share/applications/vmbby.desktop"

# ------------------------------------------------------------------------------
# Completion Notification and Path Verification Notice
# ------------------------------------------------------------------------------
echo "Installed vmbby binary to $HOME/.local/bin/vmbby"
echo "Make sure $HOME/.local/bin is present on your PATH, then launch: vmbby gui"
