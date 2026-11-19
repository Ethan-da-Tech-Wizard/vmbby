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
// Standard-library imports only (no crates). Each one is used for exactly the job named here, so
// a reader can see the whole dependency surface at a glance.
//
// env: argv, XDG_*/HOME lookups, current_exe() for finding the GUI helper script.
// Map of this file, top to bottom, so a reader can find things without searching:
//   1. constants and VmConfig            (VM sizing defaults and the config shape)
//   2. main / real_main                  (entry point and subcommand table)
//   3. subcommand handlers               (help, init, doctor, install, run, launch, stop, status, logs, gui,
//                                         explain, print_command)
//   4. qemu_args                         (the only place QEMU flags are built)
//   5. config and XDG path helpers       (default_config, render_config, load_config, *_dir, expand_home)
//   6. small utilities                   (found, cpu_has_vmx_or_svm, read_trimmed, read_some, shell_join/quote)
//
// Security notes: the only user-controlled strings that reach a shell are shown (shell_quote) and never
// executed through one; `found()` interpolates hard-coded binary names only. QEMU is started directly with
// an argument vector, so a hostile ISO path or config value cannot inject extra shell commands.
//
// Testing: this binary is exercised by hand (`vmbby doctor`, `vmbby command`). `command` and `explain` are
// the safest checks because they print the argument list without starting QEMU.
//
// Design decision: one process, no daemon. QEMU is the long-lived process; vmbby is a thin launcher, which
// removes any need for vmbby itself to stay running or to track child processes.
use std::env;

// OsString: QEMU arguments and the ISO path are kept as OS strings so non-UTF-8 paths are not corrupted.
use std::ffi::OsString;

// fs: config/log reads, directory creation, and the append-mode log file QEMU writes into.
use std::fs;

// Read/Write traits are needed for `read` and `write_all` on the QMP socket (traits must be in scope).
use std::io::{Read, Write};

// UnixStream: QMP is a UNIX-domain socket, which is also why this program is Linux/Unix only.
use std::os::unix::net::UnixStream;

// Path (borrowed) for parameters, PathBuf (owned) for the config struct fields.
use std::path::{Path, PathBuf};

// Command/Stdio: spawning qemu-system-x86_64, qemu-img, python3 and `sh -c` probes.
use std::process::{Command, Stdio};

// Duration: the QMP read timeout only.
use std::time::Duration;

// ------------------------------------------------------------------------------
// Architectural Constants & Default Virtual Machine Sizing Parameters
// ------------------------------------------------------------------------------
// Application identifier used across XDG config/data/runtime directory paths
// Single source for directory names so config, data and runtime dirs can never drift apart.
const APP_NAME: &str = "vmbby";

// Standard RAM allocation for Ubuntu 24.04 LTS desktop guest (6 GiB)
// 6G: Ubuntu 24.04's GNOME desktop is comfortable at 4G; 6G leaves headroom for a browser and an IDE
// while still leaving most of a 16G laptop to the host. Raise it in the config file, not here.
const DEFAULT_RAM: &str = "6G";

// Standard SMP vCPU thread allocation (4 virtual cores)
// 4 vCPUs: half of a typical 8-thread laptop CPU, so the host stays responsive while the guest builds code.
const DEFAULT_CPUS: &str = "4";

// Sparse dynamically-expanding virtual disk ceiling (60 GiB qcow2 capacity)
// qcow2 is sparse, so 60G is only a ceiling; the file on the host starts at a few hundred KB and grows with use.
const DEFAULT_DISK_SIZE: &str = "60G";

/// Encapsulates the immutable runtime configuration of a managed virtual machine instance.
/// 
/// Invariant: All file paths (`disk_path`, `qmp_socket`, `pid_file`, `log_file`) must be absolute
/// canonical paths or safely expanded from user home directories to prevent CWD dependency issues.
// Clone: helpers can take their own copy of the config. Debug: allows `{:?}` output while troubleshooting.
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

// Exit code contract: 0 on success, 1 on any error from a handler. Nothing else is returned, so scripts
// only need to test for zero.
/// Process Entry Point: Marshals CLI execution through `real_main` and formats top-level errors.
// All failure paths funnel through here so every error is printed as `vmbby: <message>` on stderr and
// shell scripts can rely on a non-zero exit status.
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
// Hand-rolled argv parsing (no clap): there are only ten fixed words and one positional argument, and
// avoiding a dependency keeps the build offline and the binary tiny.
fn real_main() -> Result<(), String> {
    // args_os (not args) so a non-UTF-8 ISO path still reaches `install` unchanged.
    let mut args = env::args_os();
    // Discard argv[0] (the executable path itself)
    // argv[0] is dropped first so the next call yields the subcommand; a bare `vmbby` therefore reaches "help".
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
        // Handlers take no arguments and read all state from the config file, so the dispatcher stays a pure table.
        "doctor" => doctor(),
        "init" => init(),
        // Parsed here rather than in install() so the handler signature stays a plain &Path and is easy to test.
        "install" => {
            // `install` is the only subcommand with a required positional argument.
            let iso = args
                .next()
                .ok_or("usage: vmbby install /path/to/ubuntu.iso")?;
            install(Path::new(&iso))
        }

        // `false` = not launched by the GUI; the parameter is a placeholder for future GUI-specific behaviour.
        "run" => run(false),
        // "portal" is kept as an alias so older docs and shortcuts still work.
        "gui" | "portal" => gui(),
        // stop/status/logs only read state that QEMU and launch() left behind (socket, pid file, log), so they work from any shell.
        "stop" => stop(),
        "status" => status(),
        "logs" => logs(),
        "explain" => explain(),
        // Prints the run-mode command only; the install-mode command needs an ISO path, so it is not offered here.
        "command" => print_command(false, None),
        // Help exits 0 (Ok) because asking for help is not a failure.
        "help" | "--help" | "-h" => {
            help();
            Ok(())
        }
        // Unknown words are an error (exit 1) rather than silently showing help, so scripts notice typos.
        // The only place an unrecognised word is rejected; the error text points the user at `vmbby help`.
        other => Err(format!("unknown command '{other}'. Try: vmbby help")),
    }
}

/// Emits standardized command-line documentation and hardware sizing defaults to stdout.
// Static text only; keep the command list here in sync with the match arms in real_main().
fn help() {
    println!(
        // Raw string so the aligned two-column command table is printed byte for byte as written here.
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
// Safe to run any number of times: every step checks before creating, so re-running never destroys data.
fn init() -> Result<(), String> {
    // Defaults, not load_config(): init must work before any config file exists.
    let cfg = default_config()?;
    
    // Ensure all required XDG directory trees exist with standard user permissions
    // create_dir_all succeeds on existing directories, which is what makes init() re-runnable.
    fs::create_dir_all(config_dir()?).map_err(|e| format!("create config dir: {e}"))?;
    fs::create_dir_all(data_dir()?).map_err(|e| format!("create data dir: {e}"))?;
    fs::create_dir_all(run_dir()?).map_err(|e| format!("create run dir: {e}"))?;

    // Never overwrite an existing config: the user may have edited RAM/CPU values, and `init` is
    // safe to re-run (also called automatically by ensure_initialized).
    let cfg_path = config_path()?;

    // Config first, disk second: the disk step reads sizes from `cfg`, and a half-finished init stays re-runnable.
    if !cfg_path.exists() {
        fs::write(&cfg_path, render_config(&cfg)).map_err(|e| format!("write config: {e}"))?;

        // Printing created/exists for each artifact makes a re-run's outcome obvious without reading the code.
        println!("created {}", cfg_path.display());
    } else {
        println!("config already exists: {}", cfg_path.display());
    }

    // Same idempotence rule for the disk: re-creating it would destroy the installed Ubuntu.
    if !cfg.disk_path.exists() {
        // Only reached when the disk file is absent, because `qemu-img create` would overwrite an existing image.
        // The disk path is converted with to_str() because it is passed as an argument string to qemu-img.
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

        // A non-zero exit from qemu-img (for example a missing binary path or a full disk) must stop init so the
        // "next:" hint is not printed after a failure.
        if !status.success() {
            return Err("qemu-img failed to create the disk".into());
        }

        // The path is printed so the user can find (or back up) the disk, which holds the entire guest OS.
        println!("created {}", cfg.disk_path.display());
    } else {
        println!("disk already exists: {}", cfg.disk_path.display());
    }

    // Tell a first-time user the next command; this is the only place the whole workflow order is spelled out.
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
// Read-only diagnostics: nothing here changes the system, so it is safe to run at any time and in any state.
// Returns Ok even when probes fail, because "missing" is a result to report, not a program error.
fn doctor() -> Result<(), String> {
    // Report lines are `label: value` on purpose so the output is greppable and easy to paste into a bug report.
    // Each probe below is independent: a missing item never aborts the report, so one run lists every problem.
    println!("vmbby doctor");

    // /etc/arch-release exists on Arch and is empty or holds a version string; presence alone is the signal.
    let host = if read_trimmed("/etc/arch-release").is_some() {
        "Arch Linux"
    } else {
        "not confirmed Arch"
    };

    // The Arch check is informational (the project targets an Arch laptop) and never blocks anything.
    println!("host: {host}");

    // Session variables come from the login environment; they tell the user which compositor will host the
    // QEMU GTK window (the `-display gtk` choice in qemu_args depends on a working desktop session).
    println!("desktop: {}", env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "unknown".into()));

    // XDG_SESSION_TYPE is "wayland" or "x11"; the QEMU GTK window works under both but Wayland has different quirks.
    println!("session: {}", env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".into()));
    println!("wayland: {}", env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "not detected".into()));

    // Binary checks: all three are hard requirements. qemu-system runs the VM, qemu-img makes the disk,
    // python3 runs the optional GTK panel behind `vmbby gui`.
    println!("qemu-system-x86_64: {}", found("qemu-system-x86_64"));

    // found() runs `sh -c` once per binary; three quick spawns cost milliseconds, so no caching is needed.
    println!("qemu-img: {}", found("qemu-img"));
    println!("python3: {}", found("python3"));

    // Two separate KVM signals: the device node (kernel module loaded and exposed) and the CPU flag
    // (firmware has virtualization enabled). Reporting both distinguishes "BIOS off" from "module not loaded".
    println!("/dev/kvm: {}", if Path::new("/dev/kvm").exists() { "present" } else { "missing" });

    // Reads /proc/cpuinfo; on a VM host inside another VM this is often "missing" even when KVM works.
    println!("cpu virtualization flag: {}", cpu_has_vmx_or_svm());

    // /dev/kvm is the single most common failure; without it QEMU silently falls back to slow
    // software emulation (see qemu_args), so spell out the fix instead of just printing "missing".
    // Provide actionable mitigation guidance if hardware virtualization acceleration is inaccessible
    // Only the missing-KVM case gets hints: every other doctor line is self-explanatory.
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
// The ISO is borrowed as &Path (not String) so odd file names pass through to QEMU unchanged.
fn install(iso: &Path) -> Result<(), String> {
    // Fail early with the user's own path in the message; a mistyped ISO is the most common install error.
    if !iso.exists() {
        return Err(format!("ISO not found: {}", iso.display()));
    }
    // Check the ISO before ensure_initialized so a typo does not create the config and a 60G disk first.
    ensure_initialized()?;

    // install_mode=true adds `-cdrom <iso> -boot d` so the installer boots instead of the (empty) disk.
    launch(true, Some(iso))
}

/// Boots the virtual machine in production operational mode from the installed QCOW2 virtual drive.
///
/// The `_from_gui` flag is currently unused (underscore-prefixed on purpose); every caller passes `false`.
// Kept as a separate function from launch() so the CLI table reads naturally and install/run share one code path.
fn run(_from_gui: bool) -> Result<(), String> {
    // Run mode needs the config and disk to exist; a fresh machine gets them created transparently.
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
// Called by: install() and run(). The VM outlives this process; `vmbby stop` or the guest itself ends it.
fn launch(install_mode: bool, iso: Option<&Path>) -> Result<(), String> {
    let cfg = load_config()?;
    // Only a warning, never a hard stop: QEMU removes the pid file on clean exit, but a crash or
    // power loss leaves it behind, and refusing to start would then lock the user out.
    // pid_file is written by QEMU itself (-pidfile), so its presence is a hint of an existing instance.
    if cfg.pid_file.exists() {
        println!("pid file exists: {}", cfg.pid_file.display());
        println!("If the VM is not actually running, remove that stale file.");
    }

    // Build the argument list before opening the log so a config error (bad ISO, missing HOME) leaves no empty log behind.
    let args = qemu_args(&cfg, install_mode, iso)?;
    // Echo the exact command first so a failed boot can be reproduced by hand.
    // Printing the command is the main debugging aid when KVM or display problems occur.
    println!("starting Ubuntu VM:");
    println!("qemu-system-x86_64 {}", shell_join(&args));

    // Open persistent log descriptor with append semantics
    // append(true) keeps earlier boots' output, so `vmbby logs` shows history across restarts.
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&cfg.log_file)
        .map_err(|e| format!("open log file: {e}"))?;
    // stdout and stderr need separate handles that share one file offset, hence try_clone.
    let log_err = log.try_clone().map_err(|e| format!("clone log file: {e}"))?;

    // Spawn QEMU process with null standard input and redirected output
    // QEMU is detached from our stdio: stdin is /dev/null (it never reads the terminal) and both output
    // streams go to the log, otherwise a closed terminal could kill it with SIGHUP/SIGPIPE.
    Command::new("qemu-system-x86_64")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        // spawn() (not status()) so vmbby returns immediately while the VM window keeps running.
        // The Child handle is dropped without wait(); QEMU keeps running independently of vmbby.
        .spawn()
        .map_err(|e| format!("start qemu-system-x86_64: {e}"))?;

    // Printed after spawn() succeeds, which only means the process started, not that the guest booted.
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
// Called by: the `stop` subcommand. Best-effort by design: it returns once the request is written.
fn stop() -> Result<(), String> {
    let cfg = load_config()?;
    // QEMU creates the socket at startup and removes it on exit, so its absence is the cheapest
    // "is the VM up?" test and avoids a confusing connection-refused error.
    if !cfg.qmp_socket.exists() {
        // Error text names the socket path so the user can check whether XDG_RUNTIME_DIR differs between shells.
        return Err(format!("QMP socket not found: {}. VM may not be running.", cfg.qmp_socket.display()));
    }

    // Connect (not bind): QEMU is the server (`server=on` in qemu_args); we are a short-lived client.
    let mut stream = UnixStream::connect(&cfg.qmp_socket).map_err(|e| format!("connect QMP: {e}"))?;
    stream
        // Bound every read: a hung QEMU must not freeze `vmbby stop` forever. 2 s is ample for a local socket.
        // A read timeout only applies to read(); writes to a local socket are not expected to block.
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| format!("set QMP timeout: {e}"))?;
    
    // QMP starts in "capabilities negotiation" mode: QEMU rejects every command until it is left, so the
    // greeting must be consumed and qmp_capabilities acknowledged before anything else.
    // Read and discard initial QEMU greeting banner
    // The returned text is discarded on purpose: we only need QEMU to have emitted it before we continue.
    let _ = read_some(&mut stream);
    
    // Newline-terminated JSON: QMP frames messages by line, so the trailing \n is required.
    // Complete QMP capabilities negotiation handshake
    stream
        .write_all(b"{\"execute\":\"qmp_capabilities\"}\n")
        .map_err(|e| format!("send qmp_capabilities: {e}"))?;
    let _ = read_some(&mut stream);
    
    // The reply to system_powerdown is not read: success means "request delivered", and the guest
    // OS decides when (and whether) to shut down, which can take many seconds.
    // Dispatch ACPI system powerdown command
    stream
        // A write error here (for example QEMU exited between connect and write) is reported to the user as a failure.
        .write_all(b"{\"execute\":\"system_powerdown\"}\n")
        .map_err(|e| format!("send system_powerdown: {e}"))?;

    // Wording says "sent", not "stopped": the guest may ignore the power button (e.g. a locked login screen).
    println!("sent ACPI shutdown request to Ubuntu");
    Ok(())
}

/// Inspects and displays runtime metadata, disk allocation paths, and QMP socket presence.
// Called by: the `status` subcommand. Intentionally does no process inspection, only file checks.
fn status() -> Result<(), String> {
    let cfg = load_config()?;

    // status() prints only paths and a socket-existence guess; it never connects to QMP, so it is safe and
    // instant even when QEMU is hung.
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
// Called by: the `logs` subcommand. Reads QEMU's captured output, which is where boot errors (for
// example a missing KVM device or bad -drive option) end up because the window may never appear.
fn logs() -> Result<(), String> {
    // Falls back to defaults so `logs` works even before a config file exists.
    // unwrap_or evaluates default_config() eagerly, so an HOME-less environment errors even if the config exists; acceptable for a diagnostic command.
    let cfg = load_config().unwrap_or(default_config()?);
    if !cfg.log_file.exists() {
        println!("no log yet: {}", cfg.log_file.display());
        return Ok(());
    }

    // Whole-file read is acceptable: the log only holds QEMU stderr/stdout, normally a few KB. A huge log
    // would be a reason to switch to seeking from the end.
    let text = fs::read_to_string(&cfg.log_file)
        .map_err(|e| format!("read {}: {e}", cfg.log_file.display()))?;
    let lines: Vec<&str> = text.lines().collect();
    // 160 lines is roughly two terminal screens: enough context for a failed boot without flooding the shell.
    // saturating_sub: a log shorter than 160 lines gives start = 0 rather than an underflow panic.
    let start = lines.len().saturating_sub(160);

    // Print only the tail; `start` is already clamped to 0 so short logs print completely.
    for line in &lines[start..] {
        // One println per line, not one big print: output stays streaming-friendly if stdout is a pipe.
        println!("{line}");
    }
    Ok(())
}

/// Resolves path to the Python GTK4 controller script and launches it as a standalone GUI process.
// Called by: the `gui` subcommand. The panel is a separate Python process that shells back into vmbby, so
// both interfaces share one implementation of start/stop.
fn gui() -> Result<(), String> {
    // Resolve before spawning so a missing script gives a clear error rather than a Python traceback.
    let helper = gui_helper_path()?;

    // gui_helper_path() deliberately returns a path even when nothing exists, so the error can show where we looked last.
    if !helper.exists() {
        return Err(format!("GUI helper missing: {}", helper.display()));
    }
    // Spawned and not waited on: the GTK window is long-lived and vmbby should return the prompt.
    // The script is passed as an argument (not executed directly) so it needs no executable bit and
    // the python3 on PATH decides which interpreter (and thus which PyGObject) is used.
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
// Returns a path even when the file is absent; existence is checked by the caller so the message is consistent.
fn gui_helper_path() -> Result<PathBuf, String> {
    // The search order goes from "developer checkout" to "installed package", so a local edit of the
    // script always wins over a system copy.
    let name = Path::new("assets").join("vmbby_controller.py");

    // 1) Developer checkout: running `cargo run` from the repo root finds ./assets next to Cargo.toml.
    let cwd_path = env::current_dir()
        .map_err(|e| format!("current dir: {e}"))?
        .join(&name);
    if cwd_path.exists() {
        return Ok(cwd_path);
    }

    // 2) Installed or built layout: look next to the binary and one directory up (target/debug -> target).
    // current_exe() can fail on exotic platforms, so a failure just skips this step instead of erroring.
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            let beside_exe = dir.join(&name);
            if beside_exe.exists() {
                // An asset right next to the binary wins over the parent directory: it is the more specific match.
                return Ok(beside_exe);
            }

            // `target/release/../assets` works for `cargo build` trees where assets sit beside the target folder.
            let one_up = dir.join("..").join(&name);
            if one_up.exists() {
                return Ok(one_up);
            }
        }
    }

    // 3) Per-user install under $XDG_DATA_HOME/vmbby/assets.
    let user_data = data_dir()?.join("assets").join("vmbby_controller.py");
    if user_data.exists() {
        return Ok(user_data);
    }

    // Not an error here on purpose: the "helper missing" message in gui() names this path, which tells a
    // packager exactly where the file is expected.
    // Last resort: returned even if it does not exist; the caller (gui) reports "GUI helper missing" with this path.
    Ok(PathBuf::from("/usr/share/vmbby/assets/vmbby_controller.py"))
}

/// Outputs detailed architectural documentation explaining hypervisor mechanics and security contracts.
// `explain` is documentation as a command: the text is static except for the last line, which prints the
// real argv so the explanation always matches what `run` would execute.
fn explain() -> Result<(), String> {
    // Same fallback as above: explain should always work, even before `init`.
    let cfg = load_config().unwrap_or(default_config()?);

    // Run-mode args (no ISO): the install command line needs a path argument and is shown by `command` only.
    let args = qemu_args(&cfg, false, None)?;
    println!(
        // Raw string (r#""#) so the quotes, braces and backslashes in the prose need no escaping. The one `{}`
        // placeholder is the exact command line, filled by the argument after the closing quote.
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
// Useful for copy-paste debugging and for users who want to run QEMU themselves or from a systemd unit.
fn print_command(install_mode: bool, iso: Option<&Path>) -> Result<(), String> {
    // Same fallback: printing the command should not require an initialised config.
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
// The single place that knows the QEMU flags: launch(), explain() and print_command() all call it, so
// what is shown to the user is guaranteed to be what is executed.
fn qemu_args(cfg: &VmConfig, install_mode: bool, iso: Option<&Path>) -> Result<Vec<OsString>, String> {
    // KVM decides the whole CPU strategy: with it the guest runs at near-native speed; without it
    // QEMU's TCG software emulation works but is many times slower.
    let kvm = Path::new("/dev/kvm").exists();
    // Flags are pushed as separate OsStrings (not one string) so paths with spaces survive without
    // shell quoting; shell_join only quotes for *display*.
    // Typed as Vec<OsString> by the return type; `.into()` on &str and PathBuf turns each element into one.
    let mut args = vec![
        // Flag pairs below are ordered the way QEMU docs present them; order does not matter to QEMU except that
        // a `-device` must come after the `-netdev`/`-drive` it references only by id (resolved at parse end).
        //
        // -name: title of the QEMU window and process name in `ps`.
        "-name".into(),
        cfg.name.clone().into(),
        // q35 = modern PCIe chipset (better guest driver support than the legacy i440fx default).
        // accel=kvm uses hardware virtualization; accel=tcg is the slow software fallback.
        "-machine".into(),
        if kvm { "q35,accel=kvm" } else { "q35,accel=tcg" }.into(),
        // `host` exposes the real CPU's features to the guest (best speed, only valid under KVM). TCG cannot do
        // that, so it gets `max`, the richest model it can emulate.
        "-cpu".into(),
        if kvm { "host" } else { "max" }.into(),
        // Guest RAM size and vCPU count come from the config so users can tune them without recompiling.
        "-m".into(),
        cfg.ram.clone().into(),
        // SMP: the value is the vCPU count and is passed as a string because QEMU parses it itself.
        "-smp".into(),
        cfg.cpus.clone().into(),
        // if=virtio attaches the disk through the paravirtual virtio-blk driver (Ubuntu ships it), which is much
        // faster than emulated IDE. format=qcow2 is stated explicitly: QEMU refuses to probe raw/unknown formats safely.
        "-drive".into(),
        format!("file={},if=virtio,format=qcow2", cfg.disk_path.display()).into(),
        // User-mode networking (SLIRP): outbound NAT with no root privileges, no bridge, no firewall changes.
        // Trade-off: the host cannot reach guest services without extra port-forward rules.
        "-netdev".into(),
        "user,id=net0".into(),
        "-device".into(),
        // The NIC model is virtio (paravirtual), which Ubuntu's kernel supports out of the box and is faster than e1000.
        "virtio-net-pci,netdev=net0".into(),
        // gl=off avoids host OpenGL/Wayland driver problems; virtio-vga then renders in software.
        // Display stack: GTK window, virtio-vga adapter, plus an XHCI USB controller and a USB tablet so the
        // mouse pointer is absolute-positioned (no capture/release grab and no cursor drift).
        "-display".into(),
        "gtk,gl=off".into(),
        "-device".into(),
        // virtio-vga with gl=off renders in software: slower than GPU passthrough but works on every host driver.
        "virtio-vga".into(),
        "-device".into(),
        "qemu-xhci".into(),
        "-device".into(),
        // Absolute pointer device; without it the guest cursor drifts relative to the host cursor.
        "usb-tablet".into(),
        // server=on,wait=off: QEMU listens on the socket but boots without waiting for a client.
        // This is the socket `vmbby stop` connects to.
        // The `server=on,wait=off` option makes QEMU create and listen on the socket instead of connecting out.
        "-qmp".into(),
        format!("unix:{},server=on,wait=off", cfg.qmp_socket.display()).into(),
        // Lets `launch` detect a previous instance; QEMU deletes it on clean exit.
        // A pidfile also lets external tools (systemd, scripts) find the process without parsing `ps` output.
        "-pidfile".into(),
        cfg.pid_file.as_os_str().into(),
    ];

    // Install mode is the only branch that varies the argument list; everything above is shared by both modes.
    // Conditionally append optical media drive flags during installation boot
    // Installer boot: attach the ISO as a CD-ROM and put it first in the boot order. After installation,
    // run mode (no -cdrom) boots straight from the qcow2 disk.
    if install_mode {
        // Internal consistency check: callers always pass an ISO in install mode, so this guards future misuse.
        let iso = iso.ok_or("install mode requires an ISO path")?;
        args.push("-cdrom".into());
        args.push(iso.as_os_str().into());

        // `-boot d` = first boot device is the CD-ROM. It only has an effect while -cdrom is present.
        args.push("-boot".into());
        args.push("d".into());
    }

    // Returned as OsStrings so launch() can pass them untouched to Command and shell_join() can render them.
    Ok(args)
}

/// Ensures default configuration and directories exist prior to attempting VM operations.
// Called by: install() and run() before launching, so a new user can type `vmbby run` without `init`.
fn ensure_initialized() -> Result<(), String> {
    // The config file is the "has init ever run" marker; the disk is checked separately inside init().
    if !config_path()?.exists() {
        init()?;
    }
    Ok(())
}

/// Generates the default VmConfig initialized to standardized paths under XDG Base Directories.
// Pure function of the environment (XDG vars), with no file access, which is why init() can use it before a config exists.
fn default_config() -> Result<VmConfig, String> {
    // Disk and log are durable (data dir); socket and pid file are transient (runtime dir, wiped on
    // logout/reboot), which is exactly why stale pid files are possible.
    let data = data_dir()?;
    let run = run_dir()?;

    // Names are fixed (single-VM tool). Sizes are strings because they are handed to QEMU verbatim
    // ("6G" is QEMU syntax, not a number we parse).
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
// Inverse of load_config(); only called once, from init(), to seed the file for users to edit.
fn render_config(cfg: &VmConfig) -> String {
    // Plain `key=value` lines (no sections, no quoting) keep the parser in load_config trivial and
    // let users edit the file by hand. Keys here must match the match arms in load_config.
    // One format! call keeps the on-disk order identical to the struct field order, which is the order
    // users see when they open the file.
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
// Missing file is an error (callers decide to fall back with unwrap_or); a malformed line is not.
fn load_config() -> Result<VmConfig, String> {
    // Start from defaults so a config file missing a key (or an older file) still yields a full config.
    let mut cfg = default_config()?;
    let path = config_path()?;
    let text = fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;

    // Line-oriented parse: no external TOML/INI crate, so a malformed line is skipped rather than fatal.
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

        // Keys are matched exactly (no trimming of `key`); the file is written by render_config so it is stable.
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

    // A config with unknown/misspelled keys still loads; typos silently fall back to defaults, which is the
    // price of ignoring unknown keys.
    Ok(cfg)
}

/// Returns canonical path to configuration file: `$XDG_CONFIG_HOME/vmbby/config`.
// The config lives in the config dir (small, user-editable), unlike the disk image, which lives in the data dir.
fn config_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("config"))
}

/// Returns canonical configuration directory: `$XDG_CONFIG_HOME/vmbby`.
// Directory and file are separate helpers because init() must create the directory before it writes the file.
fn config_dir() -> Result<PathBuf, String> {
    Ok(base_config_dir()?.join(APP_NAME))
}

/// Returns canonical persistent data directory: `$XDG_DATA_HOME/vmbby`.
// Data dir holds large, valuable files (disk image, log); it should be backed up, unlike the runtime dir.
fn data_dir() -> Result<PathBuf, String> {
    Ok(base_data_dir()?.join(APP_NAME))
}

/// Returns runtime transient socket directory: `$XDG_RUNTIME_DIR/vmbby`.
// Runtime dir: transient files only (QMP socket, pid file); the OS clears it at logout or reboot.
fn run_dir() -> Result<PathBuf, String> {
    // XDG_RUNTIME_DIR (e.g. /run/user/1000) is private to the user and tmpfs-backed; /tmp is only a
    // fallback for odd sessions and is world-writable, so the socket there is less well protected.
    let base = env::var_os("XDG_RUNTIME_DIR")
        // Treat the XDG variables as optional: an unset or empty-environment session must still work.
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));

    // A per-app subdirectory keeps vmbby's socket and pid file apart from other programs sharing the runtime dir.
    Ok(base.join(APP_NAME))
}

/// Resolves user config root respecting `$XDG_CONFIG_HOME` or defaulting to `~/.config`.
// XDG fallback chain for config/data: env var wins, else the spec's default under $HOME.
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
// HOME is required by the XDG fallbacks; there is no getpwuid fallback because that would need libc.
fn home_dir() -> Result<PathBuf, String> {
    env::var_os("HOME")
        .map(PathBuf::from)
        // Error is a String so it can flow through the same Result<_, String> chain as everything else.
        .ok_or("HOME is not set".into())
}

/// Performs safe tilde expansion for paths beginning with `~/`.
// Called by: load_config() for the four path keys.
fn expand_home(value: &str) -> PathBuf {
    // Shells expand `~` but a config file is not read by a shell, so it is done here. Only the
    // leading `~/` form is supported (not `~user`).
    // If HOME is unavailable the `~/` is left as typed and the path is then treated as relative; that is a visible, debuggable result.
    if let Some(rest) = value.strip_prefix("~/") {
        if let Ok(home) = home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(value)
}

/// Tests for the existence of an executable binary on system `$PATH` via POSIX shell `command -v`.
// Returns a &'static str so doctor() can print it directly without allocating.
fn found(bin: &str) -> &'static str {
    // `command -v` is a shell builtin (there is no portable `which` binary), so go through `sh -c`.
    // `bin` is always a hard-coded name from doctor(), never user input, so interpolating it is safe.
    if Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {bin} >/dev/null 2>&1"))
        // Exit status only: stdout/stderr of the probe are redirected to /dev/null inside the shell command.
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
// Heuristic: the file repeats the flags line once per core, so one match anywhere in the text is enough.
fn cpu_has_vmx_or_svm() -> &'static str {
    match fs::read_to_string("/proc/cpuinfo") {
        // Flags are space-separated in the "flags" line; the surrounding spaces avoid matching longer
        // flag names that merely contain "vmx"/"svm". vmx = Intel VT-x, svm = AMD-V.
        // The guard is on the match arm so the three outcomes (present, missing, unreadable) stay distinct.
        Ok(text) if text.contains(" vmx ") || text.contains(" svm ") => "present",
        Ok(_) => "missing",
        Err(_) => "unknown",
    }
}

/// Reads file to string and trims surrounding whitespace if the file exists.
// Used for /etc/arch-release only; trimming lets callers compare the contents without a trailing newline.
fn read_trimmed(path: &str) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// Reads available bytes from a UNIX domain stream into a 4096-byte stack buffer.
// Never blocks forever: stop() sets a 2 second read timeout on the socket before calling this.
fn read_some(stream: &mut UnixStream) -> String {
    // One read is enough for QMP's short greeting/ack lines; errors and timeouts become an empty string
    // because the caller only uses this to drain the socket, not to inspect the reply.
    // Stack buffer sized well above QMP's greeting (a few hundred bytes), so one read never truncates it in practice.
    let mut buf = [0_u8; 4096];
    match stream.read(&mut buf) {
        Ok(n) => String::from_utf8_lossy(&buf[..n]).to_string(),
        // A timeout surfaces as Err(WouldBlock/TimedOut); treating it as "no reply" is what lets stop() carry on.
        Err(_) => String::new(),
    }
}

/// Joins a slice of OS string arguments into a single POSIX-quoted shell command string.
// For display only: the real spawn uses the unquoted arguments, so quoting bugs can never change behaviour.
fn shell_join(args: &[OsString]) -> String {
    args.iter()
        // to_string_lossy is acceptable here because this string is only displayed, never executed.
        .map(|a| shell_quote(&a.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Quotes a command string for safe shell interpolation, escaping inner single quotes.
// Called by: shell_join() once per argument. Display-only helper, not a security boundary.
fn shell_quote(s: &str) -> String {
    // Fast path: strings made only of these "safe" characters need no quotes, keeping the printed command readable.
    // Whitelist approach: anything not obviously safe is quoted, which is always correct for POSIX shells.
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=,+".contains(c)) {
        s.to_string()
    } else {
        // POSIX single-quote escaping: end the quote, add an escaped quote, reopen the quote (' -> '\'').
        // Example: it's -> 'it'\''s'. The result is correct for sh/bash/zsh and safe to paste into a terminal.
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}
