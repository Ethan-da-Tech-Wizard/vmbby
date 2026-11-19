// ==============================================================================
// Program: main.rs
// Programmer: Ethan-da-Tech-Wizard
// Last Modified: November 19, 2026
// Purpose: Core Rust CLI engine and hypervisor orchestrator for VMBby.
//          Interacts with QEMU/KVM via QMP (QEMU Machine Protocol) unix sockets,
//          manages copy-on-write qcow2 disk overlays, allocates vCPUs and RAM,
//          and handles graceful guest shutdown and hardware health diagnostics.
//          Enforces memory safety, process isolation, and asynchronous signal handling.
// ==============================================================================

// How this file fits the whole app: vmbby is a single-binary CLI. This file owns every subcommand
// (`doctor`, `init`, `install`, `run`, `gui`, `stop`, `status`, `logs`, `explain`, `command`). It shells
// out to `qemu-system-x86_64` and `qemu-img`, talks QMP over a UNIX socket to stop the guest, and
// starts `assets/vmbby_controller.py` (a Python GTK4 panel) for `vmbby gui`. No crates are used, so
// the whole dependency surface is the Rust standard library plus the host's QEMU install.
//
// Inputs/outputs: argv subcommand in; human-readable text on stdout, errors on stderr with exit
// code 1. Persistent state lives only in XDG dirs: config file, qcow2 disk, log file, and (runtime
// dir) the QMP socket and QEMU pid file. Every fallible helper returns `Result<_, String>` so
// messages reach the user unchanged.
//
// Known limits: Linux only (uses /dev/kvm, /proc/cpuinfo, UNIX sockets); no automated tests (the
// behaviour depends on a real QEMU); `vmbby stop` fires and forgets (it does not wait for the guest
// to actually power off). Run `vmbby doctor` first on a new machine.
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

// ------------------------------------------------------------------------------
// Architectural Constants & Default Virtual Machine Sizing Parameters
// ------------------------------------------------------------------------------
// Application identifier used across XDG config/data/runtime directory paths
const APP_NAME: &str = "vmbby";

// Standard RAM allocation for Ubuntu 24.04 LTS desktop guest (6 GiB)
const DEFAULT_RAM: &str = "6G";

// Standard SMP vCPU thread allocation (4 virtual cores)
const DEFAULT_CPUS: &str = "4";

// Sparse dynamically-expanding virtual disk ceiling (60 GiB qcow2 capacity)
const DEFAULT_DISK_SIZE: &str = "60G";

/// Encapsulates the immutable runtime configuration of a managed virtual machine instance.
/// 
/// Invariant: All file paths (`disk_path`, `qmp_socket`, `pid_file`, `log_file`) must be absolute
/// canonical paths or safely expanded from user home directories to prevent CWD dependency issues.
#[derive(Clone, Debug)]
struct VmConfig {
    /// Human-readable virtual machine instance name passed to QEMU `-name`
    name: String,
    /// Memory sizing specification string passed to `-m` (e.g., "6G")
    ram: String,
    /// Symmetric multiprocessing core count passed to `-smp` (e.g., "4")
    cpus: String,
    /// Virtual drive capacity string passed to `qemu-img` (e.g., "60G")
    disk_size: String,
    /// Absolute filesystem path to the copy-on-write QCOW2 virtual disk image
    disk_path: PathBuf,
    /// UNIX domain socket path used for bidirectional QMP JSON-RPC communications
    qmp_socket: PathBuf,
    /// Process ID lockfile path tracking the active QEMU host PID
    pid_file: PathBuf,
    /// Standard output and error log aggregation file destination
    log_file: PathBuf,
}

/// Process Entry Point: Marshals CLI execution through `real_main` and formats top-level errors.
fn main() {
    // Execute orchestrator logic within Result context, exiting cleanly with status 1 on errors
    if let Err(err) = real_main() {
        eprintln!("vmbby: {err}");
        std::process::exit(1);
    }
}

/// Primary CLI Command Dispatcher and Argument Routing Pipeline.
/// 
/// Parses command line parameters from `std::env::args_os()` preserving native OS string
/// representations, dispatches to dedicated subcommand handlers, and returns structured errors.
fn real_main() -> Result<(), String> {
    // args_os (not args) so a non-UTF-8 ISO path still reaches `install` unchanged.
    let mut args = env::args_os();
    // Discard argv[0] (the executable path itself)
    let _bin = args.next();
    
    // Parse the primary subcommand argument, defaulting to help display if omitted
    let command = args
        .next()
        .unwrap_or_else(|| OsString::from("help"))
        .to_string_lossy()
        .to_string();
    // The subcommand name itself is plain ASCII, so a lossy conversion is safe here.

    // Match subcommand string and route to corresponding systems handler
    match command.as_str() {
        "doctor" => doctor(),
        "init" => init(),
        "install" => {
            // `install` is the only subcommand with a required positional argument.
            let iso = args
                .next()
                .ok_or("usage: vmbby install /path/to/ubuntu.iso")?;
            install(Path::new(&iso))
        }
        "run" => run(false),
        // "portal" is kept as an alias so older docs and shortcuts still work.
        "gui" | "portal" => gui(),
        "stop" => stop(),
        "status" => status(),
        "logs" => logs(),
        "explain" => explain(),
        // Prints the run-mode command only; the install-mode command needs an ISO path, so it is not offered here.
        "command" => print_command(false, None),
        "help" | "--help" | "-h" => {
            help();
            Ok(())
        }
        // Unknown words are an error (exit 1) rather than silently showing help, so scripts notice typos.
        other => Err(format!("unknown command '{other}'. Try: vmbby help")),
    }
}

/// Emits standardized command-line documentation and hardware sizing defaults to stdout.
fn help() {
    println!(
        r#"vmbby - Arch/Wayland Ubuntu VM launcher and hypervisor orchestrator

Commands:
  vmbby doctor                 Check host virtualization and environment readiness
  vmbby init                   Create XDG config hierarchy and Ubuntu qcow2 disk
  vmbby install ubuntu.iso     Boot Ubuntu installer ISO into new virtual disk
  vmbby run                    Boot the installed Ubuntu virtual machine
  vmbby gui                    Open the GTK4/Libadwaita graphical control panel
  vmbby stop                   Initiate graceful ACPI shutdown via QMP socket
  vmbby status                 Display virtual machine state and socket bindings
  vmbby logs                   Print recent QEMU hypervisor stdout/stderr logs
  vmbby explain                Explain system virtualization architecture
  vmbby command                Print raw QEMU invocation command line

Default VM Specification:
  Ubuntu 24.04 LTS target, 6G RAM, 4 vCPUs, 60G dynamic qcow2 disk
"#
    );
}

/// Provisions XDG directory structures, renders default configuration, and initializes QCOW2 storage.
/// 
/// Storage Architecture:
///   Executes `qemu-img create -f qcow2 <disk_path> <disk_size>`.
///   QCOW2 (QEMU Copy-On-Write 2) creates a sparse container file that grows dynamically
///   as the guest writes data, consuming only physical host blocks when modified.
fn init() -> Result<(), String> {
    // Defaults, not load_config(): init must work before any config file exists.
    let cfg = default_config()?;
    
    // Ensure all required XDG directory trees exist with standard user permissions
    fs::create_dir_all(config_dir()?).map_err(|e| format!("create config dir: {e}"))?;
    fs::create_dir_all(data_dir()?).map_err(|e| format!("create data dir: {e}"))?;
    fs::create_dir_all(run_dir()?).map_err(|e| format!("create run dir: {e}"))?;

    // Never overwrite an existing config: the user may have edited RAM/CPU values, and `init` is
    // safe to re-run (also called automatically by ensure_initialized).
    let cfg_path = config_path()?;
    if !cfg_path.exists() {
        fs::write(&cfg_path, render_config(&cfg)).map_err(|e| format!("write config: {e}"))?;
        println!("created {}", cfg_path.display());
    } else {
        println!("config already exists: {}", cfg_path.display());
    }

    // Same idempotence rule for the disk: re-creating it would destroy the installed Ubuntu.
    if !cfg.disk_path.exists() {
        let status = Command::new("qemu-img")
            .args([
                "create",
                "-f",
                "qcow2",
                cfg.disk_path
                    .to_str()
                    .ok_or("disk path contains invalid UTF-8")?,
                &cfg.disk_size,
            ])
            // status() waits for qemu-img, which only writes a small qcow2 header, so it returns quickly.
            .status()
            .map_err(|e| format!("run qemu-img: {e}"))?;
        if !status.success() {
            return Err("qemu-img failed to create the disk".into());
        }
        println!("created {}", cfg.disk_path.display());
    } else {
        println!("disk already exists: {}", cfg.disk_path.display());
    }

    println!("next: vmbby install /path/to/ubuntu-24.04.iso");
    Ok(())
}

/// Executes environment diagnostic inspection verifying host virtualization capabilities.
/// 
/// Checks for:
///   1. Linux distribution release strings (`/etc/arch-release`).
///   2. Active Wayland/X11 display server sessions and desktop environment variables.
///   3. System path accessibility for required binaries (`qemu-system-x86_64`, `qemu-img`, `python3`).
///   4. Availability and permissions of the Linux KVM acceleration character device (`/dev/kvm`).
///   5. Hardware virtualization CPU flags (Intel VMX or AMD SVM extensions in `/proc/cpuinfo`).
fn doctor() -> Result<(), String> {
    println!("vmbby doctor");
    let host = if read_trimmed("/etc/arch-release").is_some() {
        "Arch Linux"
    } else {
        "not confirmed Arch"
    };
    println!("host: {host}");
    println!("desktop: {}", env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "unknown".into()));
    println!("session: {}", env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".into()));
    println!("wayland: {}", env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "not detected".into()));
    println!("qemu-system-x86_64: {}", found("qemu-system-x86_64"));
    println!("qemu-img: {}", found("qemu-img"));
    println!("python3: {}", found("python3"));
    println!("/dev/kvm: {}", if Path::new("/dev/kvm").exists() { "present" } else { "missing" });
    println!("cpu virtualization flag: {}", cpu_has_vmx_or_svm());

    // /dev/kvm is the single most common failure; without it QEMU silently falls back to slow
    // software emulation (see qemu_args), so spell out the fix instead of just printing "missing".
    // Provide actionable mitigation guidance if hardware virtualization acceleration is inaccessible
    if !Path::new("/dev/kvm").exists() {
        println!();
        println!("KVM is not visible. On Arch this usually means the kvm_intel module is not loaded or this environment hides /dev/kvm.");
        println!("Useful checks: lsmod | rg '^kvm', sudo modprobe kvm_intel, then confirm your user can access /dev/kvm.");
    }

    Ok(())
}

/// Boots the virtual machine in optical media installation mode.
/// 
/// Attaches the provided ISO as an emulated IDE/SATA CD-ROM and sets boot priority to CD-ROM (`-boot d`).
fn install(iso: &Path) -> Result<(), String> {
    if !iso.exists() {
        return Err(format!("ISO not found: {}", iso.display()));
    }
    // Check the ISO before ensure_initialized so a typo does not create the config and a 60G disk first.
    ensure_initialized()?;
    launch(true, Some(iso))
}

/// Boots the virtual machine in production operational mode from the installed QCOW2 virtual drive.
///
/// The `_from_gui` flag is currently unused (underscore-prefixed on purpose); every caller passes `false`.
fn run(_from_gui: bool) -> Result<(), String> {
    ensure_initialized()?;
    launch(false, None)
}

/// Spawns the QEMU system emulator child process detached from the terminal session.
/// 
/// Process Architecture & Concurrency:
///   - Validates existence of previous PID lockfiles to detect stale or duplicate instances.
///   - Composes command line arguments utilizing KVM hardware acceleration when present.
///   - Redirects stdout and stderr streams into an append-mode logfile (`cfg.log_file`)
///     to prevent terminal pipe buffer exhaustion deadlocks.
///   - Detaches child process execution asynchronously via `Command::spawn()`.
fn launch(install_mode: bool, iso: Option<&Path>) -> Result<(), String> {
    let cfg = load_config()?;
    // Only a warning, never a hard stop: QEMU removes the pid file on clean exit, but a crash or
    // power loss leaves it behind, and refusing to start would then lock the user out.
    if cfg.pid_file.exists() {
        println!("pid file exists: {}", cfg.pid_file.display());
        println!("If the VM is not actually running, remove that stale file.");
    }

    let args = qemu_args(&cfg, install_mode, iso)?;
    // Echo the exact command first so a failed boot can be reproduced by hand.
    println!("starting Ubuntu VM:");
    println!("qemu-system-x86_64 {}", shell_join(&args));

    // Open persistent log descriptor with append semantics
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&cfg.log_file)
        .map_err(|e| format!("open log file: {e}"))?;
    // stdout and stderr need separate handles that share one file offset, hence try_clone.
    let log_err = log.try_clone().map_err(|e| format!("clone log file: {e}"))?;

    // Spawn QEMU process with null standard input and redirected output
    Command::new("qemu-system-x86_64")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        // spawn() (not status()) so vmbby returns immediately while the VM window keeps running.
        // The Child handle is dropped without wait(); QEMU keeps running independently of vmbby.
        .spawn()
        .map_err(|e| format!("start qemu-system-x86_64: {e}"))?;

    println!("Ubuntu window should open now. Logs: {}", cfg.log_file.display());
    Ok(())
}

/// Signals the guest virtual machine to initiate a graceful operating system shutdown.
/// 
/// QMP (QEMU Machine Protocol) Architecture:
///   1. Connects to the local UNIX domain socket exposed by the QEMU instance (`cfg.qmp_socket`).
///   2. Capability Negotiation Handshake:
///      Upon connection, QEMU sends a greeting banner. The client must reply with
///      `{"execute": "qmp_capabilities"}` before QEMU accepts operational commands.
///   3. ACPI Powerdown Injection:
///      Issues `{"execute": "system_powerdown"}`, which instructs the virtualized
///      ACPI chipset to assert an ACPI power button event in the guest kernel.
///      The guest OS (e.g. systemd inside Ubuntu) handles this via `systemd-logind`
///      to flush disk buffers, terminate services gracefully, and power off without data loss.
fn stop() -> Result<(), String> {
    let cfg = load_config()?;
    // QEMU creates the socket at startup and removes it on exit, so its absence is the cheapest
    // "is the VM up?" test and avoids a confusing connection-refused error.
    if !cfg.qmp_socket.exists() {
        return Err(format!("QMP socket not found: {}. VM may not be running.", cfg.qmp_socket.display()));
    }

    let mut stream = UnixStream::connect(&cfg.qmp_socket).map_err(|e| format!("connect QMP: {e}"))?;
    stream
        // Bound every read: a hung QEMU must not freeze `vmbby stop` forever. 2 s is ample for a local socket.
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| format!("set QMP timeout: {e}"))?;
    
    // Read and discard initial QEMU greeting banner
    let _ = read_some(&mut stream);
    
    // Complete QMP capabilities negotiation handshake
    stream
        .write_all(b"{\"execute\":\"qmp_capabilities\"}\n")
        .map_err(|e| format!("send qmp_capabilities: {e}"))?;
    let _ = read_some(&mut stream);
    
    // The reply to system_powerdown is not read: success means "request delivered", and the guest
    // OS decides when (and whether) to shut down, which can take many seconds.
    // Dispatch ACPI system powerdown command
    stream
        .write_all(b"{\"execute\":\"system_powerdown\"}\n")
        .map_err(|e| format!("send system_powerdown: {e}"))?;
    println!("sent ACPI shutdown request to Ubuntu");
    Ok(())
}

/// Inspects and displays runtime metadata, disk allocation paths, and QMP socket presence.
fn status() -> Result<(), String> {
    let cfg = load_config()?;
    println!("config: {}", config_path()?.display());
    println!("disk: {}", cfg.disk_path.display());
    println!("qmp: {}", cfg.qmp_socket.display());
    println!("pid file: {}", cfg.pid_file.display());
    println!("log: {}", cfg.log_file.display());
    // Heuristic only: the socket can outlive a crashed QEMU, hence "probably".
    println!("vm state: {}", if cfg.qmp_socket.exists() { "probably running" } else { "not running or no QMP socket" });
    Ok(())
}

/// Reads and displays the trailing 160 lines of the hypervisor log file.
/// 
/// Memory Efficiency: Collects lines in memory and uses saturating subtraction to prevent underflow.
fn logs() -> Result<(), String> {
    // Falls back to defaults so `logs` works even before a config file exists.
    let cfg = load_config().unwrap_or(default_config()?);
    if !cfg.log_file.exists() {
        println!("no log yet: {}", cfg.log_file.display());
        return Ok(());
    }
    let text = fs::read_to_string(&cfg.log_file)
        .map_err(|e| format!("read {}: {e}", cfg.log_file.display()))?;
    let lines: Vec<&str> = text.lines().collect();
    // 160 lines is roughly two terminal screens: enough context for a failed boot without flooding the shell.
    let start = lines.len().saturating_sub(160);
    for line in &lines[start..] {
        println!("{line}");
    }
    Ok(())
}

/// Resolves path to the Python GTK4 controller script and launches it as a standalone GUI process.
fn gui() -> Result<(), String> {
    let helper = gui_helper_path()?;
    if !helper.exists() {
        return Err(format!("GUI helper missing: {}", helper.display()));
    }
    // Spawned and not waited on: the GTK window is long-lived and vmbby should return the prompt.
    Command::new("python3")
        .arg(helper)
        .spawn()
        .map_err(|e| format!("start GUI helper: {e}"))?;
    Ok(())
}

/// Dynamic Asset Discovery: Locates the Python GTK4 controller asset across filesystem hierarchies.
/// 
/// Checks in order:
///   1. Current working directory `./assets/vmbby_controller.py` (local development mode).
///   2. Executable parent directory `../assets/vmbby_controller.py` (Cargo target directory layout).
///   3. User XDG data directory `~/.local/share/vmbby/assets/vmbby_controller.py` (user installation).
///   4. Global system data directory `/usr/share/vmbby/assets/vmbby_controller.py` (system packages).
fn gui_helper_path() -> Result<PathBuf, String> {
    // The search order goes from "developer checkout" to "installed package", so a local edit of the
    // script always wins over a system copy.
    let name = Path::new("assets").join("vmbby_controller.py");
    let cwd_path = env::current_dir()
        .map_err(|e| format!("current dir: {e}"))?
        .join(&name);
    if cwd_path.exists() {
        return Ok(cwd_path);
    }

    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            let beside_exe = dir.join(&name);
            if beside_exe.exists() {
                return Ok(beside_exe);
            }
            let one_up = dir.join("..").join(&name);
            if one_up.exists() {
                return Ok(one_up);
            }
        }
    }

    let user_data = data_dir()?.join("assets").join("vmbby_controller.py");
    if user_data.exists() {
        return Ok(user_data);
    }

    // Last resort: returned even if it does not exist; the caller (gui) reports "GUI helper missing" with this path.
    Ok(PathBuf::from("/usr/share/vmbby/assets/vmbby_controller.py"))
}

/// Outputs detailed architectural documentation explaining hypervisor mechanics and security contracts.
fn explain() -> Result<(), String> {
    let cfg = load_config().unwrap_or(default_config()?);
    let args = qemu_args(&cfg, false, None)?;
    println!(
        r#"vmbby explain

What this is:
  vmbby is a VM manager for your Arch laptop. It does not replace Linux,
  QEMU, KVM, or Ubuntu. It controls them.

Host vs guest:
  Host = Arch Linux, currently Plasma/Wayland or Hyprland/Wayland.
  Guest = Ubuntu installed into a virtual disk and displayed in a window.

Why this is not dual boot:
  Dual boot gives Ubuntu the physical laptop after rebooting. vmbby keeps
  Arch running and starts Ubuntu as a process managed by Arch.

Why this is not a container:
  A container shares the host kernel. This boots an Ubuntu kernel inside a
  virtual machine. That matters for workloads that expect a real Ubuntu OS.

QEMU:
  QEMU supplies the virtual PC: chipset, disk controller, network card,
  display, keyboard, timers, firmware path, and other device behavior Ubuntu
  expects from a computer.

KVM:
  KVM is the Linux kernel virtualization interface exposed through /dev/kvm.
  With KVM, normal guest CPU instructions execute on your actual Intel CPU,
  while privileged operations trap back to the host kernel for control.

Memory:
  The -m setting reserves the guest RAM budget. Ubuntu sees that as its RAM.
  The host still owns the real memory and can account for the QEMU process.

vCPUs:
  The -smp setting tells QEMU how many virtual CPUs Ubuntu sees. KVM schedules
  those vCPUs onto your physical CPU threads.

Disk:
  The qcow2 file is a virtual hard drive. Ubuntu thinks it is installing to a
  disk, but Arch sees one dynamically growing file.

Networking:
  User-mode NAT gives Ubuntu outbound internet without bridge setup. It is
  simple and low-risk for development work.

Wayland:
  Plasma and Hyprland are just presenting the windows. They do not create the
  VM. vmbby and QEMU do that.

Exact run command:
  qemu-system-x86_64 {}
"#,
        shell_join(&args)
    );
    Ok(())
}

/// Prints the exact command-line arguments string that would be passed to QEMU.
fn print_command(install_mode: bool, iso: Option<&Path>) -> Result<(), String> {
    let cfg = load_config().unwrap_or(default_config()?);
    println!("qemu-system-x86_64 {}", shell_join(&qemu_args(&cfg, install_mode, iso)?));
    Ok(())
}

/// Assembles the complete vector of command-line flags and parameters for invoking QEMU.
/// 
/// Virtualized Hardware Stack:
///   - Machine Model: Modern Intel Q35 PCI Express chipset (`-machine q35`).
///   - Acceleration: Kernel Virtual Machine (`accel=kvm`) with CPU pass-through (`-cpu host`),
///     or software Tiny Code Generator fallback (`accel=tcg`, `-cpu max`) if KVM is unavailable.
///   - Storage: VirtIO block device (`if=virtio`) targeting copy-on-write QCOW2 image.
///   - Networking: User-mode NAT stack (`-netdev user`) paired with VirtIO PCI NIC (`virtio-net-pci`).
///   - Display & Input: GTK3/GTK4 frontend with VirtIO-VGA accelerated framebuffer and USB XHCI tablet pointer.
///   - Telemetry IPC: QMP JSON-RPC socket server and process ID lockfile.
fn qemu_args(cfg: &VmConfig, install_mode: bool, iso: Option<&Path>) -> Result<Vec<OsString>, String> {
    // KVM decides the whole CPU strategy: with it the guest runs at near-native speed; without it
    // QEMU's TCG software emulation works but is many times slower.
    let kvm = Path::new("/dev/kvm").exists();
    // Flags are pushed as separate OsStrings (not one string) so paths with spaces survive without
    // shell quoting; shell_join only quotes for *display*.
    let mut args = vec![
        "-name".into(),
        cfg.name.clone().into(),
        "-machine".into(),
        if kvm { "q35,accel=kvm" } else { "q35,accel=tcg" }.into(),
        "-cpu".into(),
        if kvm { "host" } else { "max" }.into(),
        "-m".into(),
        cfg.ram.clone().into(),
        "-smp".into(),
        cfg.cpus.clone().into(),
        "-drive".into(),
        format!("file={},if=virtio,format=qcow2", cfg.disk_path.display()).into(),
        "-netdev".into(),
        "user,id=net0".into(),
        "-device".into(),
        "virtio-net-pci,netdev=net0".into(),
        // gl=off avoids host OpenGL/Wayland driver problems; virtio-vga then renders in software.
        "-display".into(),
        "gtk,gl=off".into(),
        "-device".into(),
        "virtio-vga".into(),
        "-device".into(),
        "qemu-xhci".into(),
        "-device".into(),
        "usb-tablet".into(),
        // server=on,wait=off: QEMU listens on the socket but boots without waiting for a client.
        // This is the socket `vmbby stop` connects to.
        "-qmp".into(),
        format!("unix:{},server=on,wait=off", cfg.qmp_socket.display()).into(),
        // Lets `launch` detect a previous instance; QEMU deletes it on clean exit.
        "-pidfile".into(),
        cfg.pid_file.as_os_str().into(),
    ];

    // Conditionally append optical media drive flags during installation boot
    if install_mode {
        // Internal consistency check: callers always pass an ISO in install mode, so this guards future misuse.
        let iso = iso.ok_or("install mode requires an ISO path")?;
        args.push("-cdrom".into());
        args.push(iso.as_os_str().into());
        args.push("-boot".into());
        args.push("d".into());
    }

    Ok(args)
}

/// Ensures default configuration and directories exist prior to attempting VM operations.
fn ensure_initialized() -> Result<(), String> {
    if !config_path()?.exists() {
        init()?;
    }
    Ok(())
}

/// Generates the default VmConfig initialized to standardized paths under XDG Base Directories.
fn default_config() -> Result<VmConfig, String> {
    // Disk and log are durable (data dir); socket and pid file are transient (runtime dir, wiped on
    // logout/reboot), which is exactly why stale pid files are possible.
    let data = data_dir()?;
    let run = run_dir()?;
    Ok(VmConfig {
        name: "vmbby-ubuntu".into(),
        ram: DEFAULT_RAM.into(),
        cpus: DEFAULT_CPUS.into(),
        disk_size: DEFAULT_DISK_SIZE.into(),
        disk_path: data.join("ubuntu.qcow2"),
        qmp_socket: run.join("vmbby-ubuntu.qmp"),
        pid_file: run.join("vmbby-ubuntu.pid"),
        log_file: data.join("qemu.log"),
    })
}

/// Serializes a VmConfig struct into key-value INI configuration file format.
fn render_config(cfg: &VmConfig) -> String {
    // Plain `key=value` lines (no sections, no quoting) keep the parser in load_config trivial and
    // let users edit the file by hand. Keys here must match the match arms in load_config.
    format!(
        "name={}\nram={}\ncpus={}\ndisk_size={}\ndisk_path={}\nqmp_socket={}\npid_file={}\nlog_file={}\n",
        cfg.name,
        cfg.ram,
        cfg.cpus,
        cfg.disk_size,
        cfg.disk_path.display(),
        cfg.qmp_socket.display(),
        cfg.pid_file.display(),
        cfg.log_file.display()
    )
}

/// Deserializes INI configuration file from disk, expanding home tildes and overriding defaults.
fn load_config() -> Result<VmConfig, String> {
    // Start from defaults so a config file missing a key (or an older file) still yields a full config.
    let mut cfg = default_config()?;
    let path = config_path()?;
    let text = fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    for line in text.lines() {
        let trimmed = line.trim();
        // Blank lines and `#` comments are skipped so users can annotate their config.
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // split_once keeps any further '=' inside the value (paths may contain them); lines with no '=' are ignored.
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        match key {
            "name" => cfg.name = value.into(),
            "ram" => cfg.ram = value.into(),
            "cpus" => cfg.cpus = value.into(),
            "disk_size" => cfg.disk_size = value.into(),
            // Only path-valued keys get `~/` expansion; size/name strings are passed to QEMU as-is.
            "disk_path" => cfg.disk_path = expand_home(value),
            "qmp_socket" => cfg.qmp_socket = expand_home(value),
            "pid_file" => cfg.pid_file = expand_home(value),
            "log_file" => cfg.log_file = expand_home(value),
            // Unknown keys are ignored for forward compatibility with newer config files.
            _ => {}
        }
    }
    Ok(cfg)
}

/// Returns canonical path to configuration file: `$XDG_CONFIG_HOME/vmbby/config`.
fn config_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("config"))
}

/// Returns canonical configuration directory: `$XDG_CONFIG_HOME/vmbby`.
fn config_dir() -> Result<PathBuf, String> {
    Ok(base_config_dir()?.join(APP_NAME))
}

/// Returns canonical persistent data directory: `$XDG_DATA_HOME/vmbby`.
fn data_dir() -> Result<PathBuf, String> {
    Ok(base_data_dir()?.join(APP_NAME))
}

/// Returns runtime transient socket directory: `$XDG_RUNTIME_DIR/vmbby`.
fn run_dir() -> Result<PathBuf, String> {
    // XDG_RUNTIME_DIR (e.g. /run/user/1000) is private to the user and tmpfs-backed; /tmp is only a
    // fallback for odd sessions and is world-writable, so the socket there is less well protected.
    let base = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    Ok(base.join(APP_NAME))
}

/// Resolves user config root respecting `$XDG_CONFIG_HOME` or defaulting to `~/.config`.
fn base_config_dir() -> Result<PathBuf, String> {
    Ok(env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or(home_dir()?.join(".config")))
}

/// Resolves user data root respecting `$XDG_DATA_HOME` or defaulting to `~/.local/share`.
fn base_data_dir() -> Result<PathBuf, String> {
    Ok(env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or(home_dir()?.join(".local/share")))
}

/// Resolves current user's `$HOME` directory from environment.
fn home_dir() -> Result<PathBuf, String> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set".into())
}

/// Performs safe tilde expansion for paths beginning with `~/`.
fn expand_home(value: &str) -> PathBuf {
    // Shells expand `~` but a config file is not read by a shell, so it is done here. Only the
    // leading `~/` form is supported (not `~user`).
    if let Some(rest) = value.strip_prefix("~/") {
        if let Ok(home) = home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(value)
}

/// Tests for the existence of an executable binary on system `$PATH` via POSIX shell `command -v`.
fn found(bin: &str) -> &'static str {
    // `command -v` is a shell builtin (there is no portable `which` binary), so go through `sh -c`.
    // `bin` is always a hard-coded name from doctor(), never user input, so interpolating it is safe.
    if Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {bin} >/dev/null 2>&1"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        "found"
    } else {
        "missing"
    }
}

/// Inspects `/proc/cpuinfo` to detect Intel VMX or AMD SVM hardware virtualization extensions.
fn cpu_has_vmx_or_svm() -> &'static str {
    match fs::read_to_string("/proc/cpuinfo") {
        // Flags are space-separated in the "flags" line; the surrounding spaces avoid matching longer
        // flag names that merely contain "vmx"/"svm". vmx = Intel VT-x, svm = AMD-V.
        Ok(text) if text.contains(" vmx ") || text.contains(" svm ") => "present",
        Ok(_) => "missing",
        Err(_) => "unknown",
    }
}

/// Reads file to string and trims surrounding whitespace if the file exists.
fn read_trimmed(path: &str) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// Reads available bytes from a UNIX domain stream into a 4096-byte stack buffer.
fn read_some(stream: &mut UnixStream) -> String {
    // One read is enough for QMP's short greeting/ack lines; errors and timeouts become an empty string
    // because the caller only uses this to drain the socket, not to inspect the reply.
    let mut buf = [0_u8; 4096];
    match stream.read(&mut buf) {
        Ok(n) => String::from_utf8_lossy(&buf[..n]).to_string(),
        Err(_) => String::new(),
    }
}

/// Joins a slice of OS string arguments into a single POSIX-quoted shell command string.
fn shell_join(args: &[OsString]) -> String {
    args.iter()
        .map(|a| shell_quote(&a.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Quotes a command string for safe shell interpolation, escaping inner single quotes.
fn shell_quote(s: &str) -> String {
    // Fast path: strings made only of these "safe" characters need no quotes, keeping the printed command readable.
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=,+".contains(c)) {
        s.to_string()
    } else {
        // POSIX single-quote escaping: end the quote, add an escaped quote, reopen the quote (' -> '\'').
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}
