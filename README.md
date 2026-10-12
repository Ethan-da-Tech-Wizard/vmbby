# VMBby

A Rust desktop and command-line launcher for Ubuntu virtual machines on an Arch Linux Wayland host.

**Status:** VM management prototype  
**Built with:** Rust, QEMU/KVM, GTK host tools

[Project guide](docs/PROJECT_GUIDE.md) · [Every file explained](docs/FILE_GUIDE.md) · [Licenses and dependencies](docs/LICENSE_REVIEW.md)

## Start here

- [Cargo.toml](Cargo.toml) — Configuration or build metadata for Cargo; read alongside the application or tool that loads it.
- [src](src) — Application source and platform integration
- [PROJECT_NOTES.md](PROJECT_NOTES.md) — Preserved detailed setup, usage, architecture, and development notes from the earlier README.

The following commands are taken from this project’s source/configuration. Install its prerequisites first.

```sh
cargo build
target/debug/vmbby doctor
```

[Detailed setup, usage, and development notes](PROJECT_NOTES.md) preserve the previous project documentation.

## Repository layout

| Path | Role |
| --- | --- |
| [assets/](assets/) | Media and visual resources used by the project (2 tracked entries). |
| [docs/](docs/) | Project documentation, decisions, and reference material (8 tracked entries). |
| [scripts/](scripts/) | Build, launch, maintenance, and verification commands (1 tracked entries). |
| [src/](src/) | Application source and platform integration (1 tracked entries). |

## Scope and current limits

This project manages QEMU/KVM; it is not a new hypervisor. Guest operating systems and VM disks are installed and stored separately.

## License

Original project code retains **MIT** terms in [LICENSE](LICENSE). Third-party libraries, media, data, models, and tools retain their own terms. See the [license review](docs/LICENSE_REVIEW.md) and [dependency inventory](docs/DEPENDENCIES.csv).
