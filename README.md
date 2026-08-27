# vmbby

`vmbby` is Ethan's personal Ubuntu VM launcher for Arch Linux on Wayland.
It is designed for KDE Plasma now and Hyprland when that is the active session.

The practical goal is simple:

```sh
vmbby run
```

That command opens a real Ubuntu VM window on the Arch desktop. The app owns the
VM workflow, configuration, lifecycle, logs, acid-trip controller visuals, and
technical explanations. QEMU/KVM provides the proven virtualization machinery.

## Commands

```sh
cargo build
target/debug/vmbby doctor
target/debug/vmbby init
target/debug/vmbby install /path/to/ubuntu-24.04.iso
target/debug/vmbby run
target/debug/vmbby gui
target/debug/vmbby explain
target/debug/vmbby stop
```

After installing the binary somewhere on `PATH`, use `vmbby` instead of
`target/debug/vmbby`.

User install:

```sh
cargo build
sh scripts/install-user.sh
vmbby gui
```

## What Is Self-Made Here

Built by this project:

- VM config and disk lifecycle
- QEMU command generation
- start, stop, status, doctor, explain commands
- Plasma/Hyprland-aware Wayland workflow
- GTK controller with psychedelic visuals
- technical documentation

Intentionally reused:

- Arch Linux as the host OS
- Linux KVM for hardware virtualization
- QEMU for the virtual PC/device model
- Ubuntu as the guest OS

That is the honest boundary. This is a self-made VMBox-style manager, not a
claim that the project rewrites QEMU or KVM.

## Default VM

- Guest: Ubuntu 24.04 LTS
- RAM: `6G`
- CPUs: `4`
- Disk: `60G` dynamic `qcow2`
- Network: QEMU user-mode NAT
- Display: QEMU GTK window
- Host desktops: KDE Plasma Wayland and Hyprland Wayland

Configuration is stored at:

```text
~/.config/vmbby/config
```

VM data is stored at:

```text
~/.local/share/vmbby/
```

Runtime sockets and pid files are stored under:

```text
$XDG_RUNTIME_DIR/vmbby/
```

## First Install Flow

1. Download the Ubuntu 24.04 LTS ISO.
2. Run `vmbby doctor`.
3. Run `vmbby init`.
4. Run `vmbby install /path/to/ubuntu.iso`.
5. Install Ubuntu onto the virtual disk shown by the installer.
6. Shut down the installer VM.
7. Run `vmbby run`.

## Visual Design

The acid-trip visuals belong to the controller, not the Ubuntu guest. That keeps
school work normal and keeps resource use sane. The controller uses GTK4 and a
small Cairo drawing loop instead of Electron.

## Roast-Proof One-Liner

I built a custom VM manager for my Ubuntu school environment on Arch. It controls
QEMU/KVM directly: config, disk lifecycle, QEMU launch command, lifecycle
commands, logs, desktop workflow, visuals, and documentation. I intentionally use
QEMU/KVM for the virtual hardware and CPU virtualization because reliable Ubuntu
support matters more than pretending I rewrote decades of virtualization code.
