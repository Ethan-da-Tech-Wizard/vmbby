# VMBby

A Rust desktop and command-line launcher for Ubuntu virtual machines on an Arch Linux Wayland host.

**Status:** VM management prototype  
**Built with:** Rust, QEMU/KVM, GTK host tools

## Reading order

1. Read the root README for project purpose and setup navigation.
2. Follow the entry points below to find the implementation.
3. Use the file guide to locate individual modules, tests, assets, and configuration.
4. Check component licenses before redistributing dependencies or assets.

## Important paths

- [Cargo.toml](../Cargo.toml) — Configuration or build metadata for Cargo; read alongside the application or tool that loads it.
- [src](../src) — Application source and platform integration
- [PROJECT_NOTES.md](../PROJECT_NOTES.md) — Preserved detailed setup, usage, architecture, and development notes from the earlier README.

## Structure

| Path | Role |
| --- | --- |
| [assets/](../assets/) | Media and visual resources used by the project (2 tracked entries). |
| [docs/](../docs/) | Project documentation, decisions, and reference material (8 tracked entries). |
| [scripts/](../scripts/) | Build, launch, maintenance, and verification commands (1 tracked entries). |
| [src/](../src/) | Application source and platform integration (1 tracked entries). |

## Useful commands

Run from the repository root after following the documented prerequisites.

```sh
cargo build
target/debug/vmbby doctor
```

## Scope

This project manages QEMU/KVM; it is not a new hypervisor. Guest operating systems and VM disks are installed and stored separately.

## Maintaining the guide

The file catalog is a source-inspection snapshot, not proof that every runtime path works. Update it when moving files or changing module responsibilities. The 2026-10-12 documentation review did not run every application, deploy services, or validate all historical performance claims.
