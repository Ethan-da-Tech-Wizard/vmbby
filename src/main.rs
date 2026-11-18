// Program: vmbby - main.rs
// Programmer: Ethan-da-Tech-Wizard (written with Claude, an AI assistant)
// Last Modified: 2026-11-18
// Purpose: Command-line program that creates, starts, stops and explains an Ubuntu virtual machine run through QEMU/KVM on an Arch Linux host.

//! vmbby is a small command-line manager for one Ubuntu virtual machine.
//! It builds a QEMU command line, starts QEMU, talks to it over a control socket
//! (QMP) to shut it down, and can open a GTK controller window written in Python.
//!
//! Usage: vmbby <doctor|init|install ISO|run|gui|stop|status|logs|explain|command|help>
// SECTION: IMPORTS (everything below comes from Rust's standard library)
// env reads environment variables and command-line arguments, OsString holds text that may not be valid UTF-8, fs reads and writes files, Read/Write are the traits for reading and writing streams, UnixStream is a local socket connection (used for QMP), Path/PathBuf are file paths, Command/Stdio start other programs, and Duration is a length of time.
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

// SECTION: CONSTANTS AND DATA TYPES
// APP_NAME names the folders vmbby uses; the DEFAULT_* values are the VM size used when no config says otherwise.
const APP_NAME: &str = "vmbby";
const DEFAULT_RAM: &str = "6G";
const DEFAULT_CPUS: &str = "4";
const DEFAULT_DISK_SIZE: &str = "60G";

/// Settings for the virtual machine, loaded from the config file or defaults.
#[derive(Clone, Debug)]
struct VmConfig {
    /// Name QEMU shows for the VM.
    name: String,
    /// Memory size in QEMU notation, such as "6G".
    ram: String,
    /// Number of virtual CPUs as text.
    cpus: String,
    /// Size used when the disk image is first created.
    disk_size: String,
    /// Location of the qcow2 virtual disk file.
    disk_path: PathBuf,
    /// Socket file QEMU listens on for control commands.
    qmp_socket: PathBuf,
    /// File where QEMU writes its process id.
    pid_file: PathBuf,
    /// File that collects QEMU's output.
    log_file: PathBuf,
}

// SECTION: PROGRAM ENTRY AND COMMAND DISPATCH
/// Program entry point: run the real work and, if it returns an error, print it to stderr and exit with code 1.
fn main() {
    // Print the error message and stop with a failure exit code.
    if let Err(err) = real_main() {
        eprintln!("vmbby: {err}");
        std::process::exit(1);
    }
}

/// Read the first command-line word and call the matching function; unknown words produce an error.
fn real_main() -> Result<(), String> {
    // Get the program's arguments; the first one is the program name, which we skip.
    let mut args = env::args_os();
    let _bin = args.next();
    // Take the next word as the command (default "help"), converting it to a normal String.
    let command = args
        .next()
        .unwrap_or_else(|| OsString::from("help"))
        .to_string_lossy()
        .to_string();

    // Pick the function for each command word.
    match command.as_str() {
        "doctor" => doctor(),
        "init" => init(),
        // install needs one more argument (the ISO file); if it is missing, return a usage message.
        "install" => {
            let iso = args
                .next()
                .ok_or("usage: vmbby install /path/to/ubuntu.iso")?;
            install(Path::new(&iso))
        }
        // run boots the installed VM; "gui" and "portal" are two names for the same command.
        "run" => run(false),
        "gui" | "portal" => gui(),
        "stop" => stop(),
        "status" => status(),
        "logs" => logs(),
        "explain" => explain(),
        "command" => print_command(false, None),
        // Help text is printed and then we return Ok.
        "help" | "--help" | "-h" => {
            help();
            Ok(())
        }
        // Any other word is an unknown command.
        other => Err(format!("unknown command '{other}'. Try: vmbby help")),
    }
}

/// Print the list of commands and the default VM size.
fn help() {
    println!(
        r#"vmbby - Ethan's Arch/Wayland Ubuntu VM launcher

Commands:
  vmbby doctor                 Check host VM readiness
  vmbby init                   Create config and Ubuntu qcow2 disk
  vmbby install ubuntu.iso     Boot Ubuntu installer ISO
  vmbby run                    Boot the installed Ubuntu VM
  vmbby gui                    Open the psychedelic controller
  vmbby stop                   Ask the VM to shut down cleanly
  vmbby status                 Show VM state
  vmbby logs                   Print recent QEMU log output
  vmbby explain                Explain exactly how this works
  vmbby command                Print the QEMU command used by run

Default VM:
  Ubuntu 24.04 LTS target, 6G RAM, 4 vCPUs, 60G dynamic qcow2 disk
"#
    );
}

// SECTION: COMMANDS
/// Create the config, data and run folders, a config file and the qcow2 disk image if they do not exist yet.
fn init() -> Result<(), String> {
    // Start with the default settings.
    let cfg = default_config()?;
    // Make each folder (and parents); on failure return a readable error message.
    fs::create_dir_all(config_dir()?).map_err(|e| format!("create config dir: {e}"))?;
    fs::create_dir_all(data_dir()?).map_err(|e| format!("create data dir: {e}"))?;
    fs::create_dir_all(run_dir()?).map_err(|e| format!("create run dir: {e}"))?;

    // Write the config file only if it is not there, so we never overwrite a user's edits.
    let cfg_path = config_path()?;
    if !cfg_path.exists() {
        fs::write(&cfg_path, render_config(&cfg)).map_err(|e| format!("write config: {e}"))?;
        println!("created {}", cfg_path.display());
    } else {
        println!("config already exists: {}", cfg_path.display());
    }

    // Create the disk only if it is missing, by running qemu-img (-f qcow2 = format, last argument = size).
    if !cfg.disk_path.exists() {
        let status = Command::new("qemu-img")
            .args([
                "create",
                "-f",
                "qcow2",
                cfg.disk_path
                    .to_str()
                    // The path must be valid UTF-8 to pass it to qemu-img.
                    .ok_or("disk path contains invalid UTF-8")?,
                &cfg.disk_size,
            ])
            .status()
            .map_err(|e| format!("run qemu-img: {e}"))?;
        // A non-zero exit from qemu-img is an error.
        if !status.success() {
            return Err("qemu-img failed to create the disk".into());
        }
        println!("created {}", cfg.disk_path.display());
    } else {
        println!("disk already exists: {}", cfg.disk_path.display());
    }

    // Tell the user the next step.
    println!("next: vmbby install /path/to/ubuntu-24.04.iso");
    Ok(())
}

/// Print a checklist of host facts: distro, desktop, session type, needed programs and KVM support.
fn doctor() -> Result<(), String> {
    println!("vmbby doctor");
    // Check for /etc/arch-release to see if the host is Arch Linux.
    let host = if read_trimmed("/etc/arch-release").is_some() {
        "Arch Linux"
    } else {
        "not confirmed Arch"
    };
    println!("host: {host}");
    // Show environment variables that describe the desktop session, or "unknown" if unset.
    println!("desktop: {}", env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "unknown".into()));
    println!("session: {}", env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".into()));
    println!("wayland: {}", env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "not detected".into()));
    // Check whether each required program is installed.
    println!("qemu-system-x86_64: {}", found("qemu-system-x86_64"));
    println!("qemu-img: {}", found("qemu-img"));
    println!("python3: {}", found("python3"));
    ///dev/kvm exists when the kernel offers hardware virtualization; the CPU flag check below looks for vmx (Intel) or svm (AMD).
    println!("/dev/kvm: {}", if Path::new("/dev/kvm").exists() { "present" } else { "missing" });
    println!("cpu virtualization flag: {}", cpu_has_vmx_or_svm());

    // When KVM is missing, print hints about how to fix it.
    if !Path::new("/dev/kvm").exists() {
        println!();
        println!("KVM is not visible. On Arch this usually means the kvm_intel module is not loaded or this environment hides /dev/kvm.");
        println!("Useful checks: lsmod | rg '^kvm', sudo modprobe kvm_intel, then confirm your user can access /dev/kvm.");
    }

    Ok(())
}

/// Boot the Ubuntu installer from an ISO file: check it exists, make sure setup was done, then launch in install mode.
fn install(iso: &Path) -> Result<(), String> {
    if !iso.exists() {
        return Err(format!("ISO not found: {}", iso.display()));
    }
    ensure_initialized()?;
    launch(true, Some(iso))
}

/// Boot the already-installed VM (the argument is unused for now).
fn run(_from_gui: bool) -> Result<(), String> {
    ensure_initialized()?;
    launch(false, None)
}

/// Build the QEMU arguments and start QEMU in the background, sending its output to the log file.
fn launch(install_mode: bool, iso: Option<&Path>) -> Result<(), String> {
    // Load saved settings.
    let cfg = load_config()?;
    // A leftover pid file may mean a VM is already running, or may be stale; we only warn.
    if cfg.pid_file.exists() {
        println!("pid file exists: {}", cfg.pid_file.display());
        println!("If the VM is not actually running, remove that stale file.");
    }

    // Build the list of QEMU arguments and print the full command for the user.
    let args = qemu_args(&cfg, install_mode, iso)?;
    println!("starting Ubuntu VM:");
    println!("qemu-system-x86_64 {}", shell_join(&args));

    // Open the log file for appending (created if missing).
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&cfg.log_file)
        .map_err(|e| format!("open log file: {e}"))?;
    // Clone the handle so both stdout and stderr can write into the same log.
    let log_err = log.try_clone().map_err(|e| format!("clone log file: {e}"))?;

    // Start QEMU with no keyboard input, output to the log; spawn() returns right away without waiting for it to finish.
    Command::new("qemu-system-x86_64")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .spawn()
        .map_err(|e| format!("start qemu-system-x86_64: {e}"))?;

    println!("Ubuntu window should open now. Logs: {}", cfg.log_file.display());
    Ok(())
}

/// Ask the VM to shut down cleanly (like pressing a power button) through the QMP control socket.
fn stop() -> Result<(), String> {
    let cfg = load_config()?;
    // No socket file means the VM is probably not running.
    if !cfg.qmp_socket.exists() {
        return Err(format!("QMP socket not found: {}. VM may not be running.", cfg.qmp_socket.display()));
    }

    // Connect to QEMU's control socket.
    let mut stream = UnixStream::connect(&cfg.qmp_socket).map_err(|e| format!("connect QMP: {e}"))?;
    stream
        // Do not wait more than 2 seconds for a reply.
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| format!("set QMP timeout: {e}"))?;
    // Read and ignore QEMU's greeting message.
    let _ = read_some(&mut stream);
    stream
        // QMP needs the qmp_capabilities command first to finish the handshake.
        .write_all(b"{\"execute\":\"qmp_capabilities\"}\n")
        .map_err(|e| format!("send qmp_capabilities: {e}"))?;
    let _ = read_some(&mut stream);
    stream
        // Then send system_powerdown, which asks the guest OS to shut down (an ACPI power button press).
        .write_all(b"{\"execute\":\"system_powerdown\"}\n")
        .map_err(|e| format!("send system_powerdown: {e}"))?;
    println!("sent ACPI shutdown request to Ubuntu");
    Ok(())
}

/// Print where the config, disk, socket, pid file and log are, and guess whether the VM is running.
fn status() -> Result<(), String> {
    let cfg = load_config()?;
    println!("config: {}", config_path()?.display());
    println!("disk: {}", cfg.disk_path.display());
    println!("qmp: {}", cfg.qmp_socket.display());
    println!("pid file: {}", cfg.pid_file.display());
    println!("log: {}", cfg.log_file.display());
    // The VM is only "probably running" because we merely check that the socket file exists.
    println!("vm state: {}", if cfg.qmp_socket.exists() { "probably running" } else { "not running or no QMP socket" });
    Ok(())
}

/// Print the last 160 lines of the QEMU log file.
fn logs() -> Result<(), String> {
    // Use saved settings, or the defaults if there is no config yet.
    let cfg = load_config().unwrap_or(default_config()?);
    if !cfg.log_file.exists() {
        println!("no log yet: {}", cfg.log_file.display());
        return Ok(());
    }
    let text = fs::read_to_string(&cfg.log_file)
        .map_err(|e| format!("read {}: {e}", cfg.log_file.display()))?;
    // Split the log into lines and compute where the last 160 begin (saturating_sub avoids going below zero).
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(160);
    for line in &lines[start..] {
        println!("{line}");
    }
    Ok(())
}

/// Open the Python GTK controller window by running the helper script with python3.
fn gui() -> Result<(), String> {
    // Find where the helper script lives.
    let helper = gui_helper_path()?;
    if !helper.exists() {
        return Err(format!("GUI helper missing: {}", helper.display()));
    }
    // Start the helper; spawn() does not wait for the window to close.
    Command::new("python3")
        .arg(helper)
        .spawn()
        .map_err(|e| format!("start GUI helper: {e}"))?;
    Ok(())
}

/// Search for the GUI helper script in several places and return the first that exists: current folder, next to the program, one folder up, the user's data folder, then the system-wide path.
fn gui_helper_path() -> Result<PathBuf, String> {
    // The file we are looking for, relative to each search folder.
    let name = Path::new("assets").join("vmbby_controller.py");
    // First choice: the current working directory.
    let cwd_path = env::current_dir()
        .map_err(|e| format!("current dir: {e}"))?
        .join(&name);
    if cwd_path.exists() {
        return Ok(cwd_path);
    }

    // Next: relative to the running program's own location.
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

    // Next: the per-user data folder.
    let user_data = data_dir()?.join("assets").join("vmbby_controller.py");
    if user_data.exists() {
        return Ok(user_data);
    }

    // Last resort: the system-wide install location (not checked for existence here).
    Ok(PathBuf::from("/usr/share/vmbby/assets/vmbby_controller.py"))
}

/// Print a plain-English explanation of VM, QEMU, KVM and the exact QEMU command this program uses.
fn explain() -> Result<(), String> {
    let cfg = load_config().unwrap_or(default_config()?);
    // Build the real run command so it can be shown at the end of the explanation.
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
  virtual machine. That matters for classes that expect a real Ubuntu OS.

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
  simple and low-risk for everyday work.

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

/// Print the QEMU command that would be used, without running it.
fn print_command(install_mode: bool, iso: Option<&Path>) -> Result<(), String> {
    let cfg = load_config().unwrap_or(default_config()?);
    println!("qemu-system-x86_64 {}", shell_join(&qemu_args(&cfg, install_mode, iso)?));
    Ok(())
}

// SECTION: QEMU COMMAND BUILDING
/// Build the list of arguments for qemu-system-x86_64, adding the installer ISO when install_mode is true.
fn qemu_args(cfg: &VmConfig, install_mode: bool, iso: Option<&Path>) -> Result<Vec<OsString>, String> {
    // KVM acceleration is only used when /dev/kvm exists; otherwise QEMU falls back to slower software emulation (tcg).
    let kvm = Path::new("/dev/kvm").exists();
    // Start the argument list. Each flag is followed by its value.
    let mut args = vec![
        "-name".into(),
        cfg.name.clone().into(),
        // -machine: virtual PC type q35 with KVM or TCG acceleration.
        "-machine".into(),
        if kvm { "q35,accel=kvm" } else { "q35,accel=tcg" }.into(),
        // -cpu: copy the host CPU with KVM, or use the generic "max" CPU in software mode.
        "-cpu".into(),
        if kvm { "host" } else { "max" }.into(),
        // -m: guest memory size.
        "-m".into(),
        cfg.ram.clone().into(),
        // -smp: number of virtual CPUs.
        "-smp".into(),
        cfg.cpus.clone().into(),
        // -drive: attach the qcow2 disk as a virtio disk.
        "-drive".into(),
        format!("file={},if=virtio,format=qcow2", cfg.disk_path.display()).into(),
        // -netdev and -device: user-mode NAT networking with a virtio network card.
        "-netdev".into(),
        "user,id=net0".into(),
        "-device".into(),
        "virtio-net-pci,netdev=net0".into(),
        // -display gtk: show the VM in a GTK window (OpenGL off); virtio-vga is the virtual graphics card.
        "-display".into(),
        "gtk,gl=off".into(),
        "-device".into(),
        "virtio-vga".into(),
        "-device".into(),
        // A USB controller plus a USB tablet so the mouse pointer behaves smoothly.
        "qemu-xhci".into(),
        "-device".into(),
        "usb-tablet".into(),
        // -qmp: open the control socket that stop() talks to.
        "-qmp".into(),
        format!("unix:{},server=on,wait=off", cfg.qmp_socket.display()).into(),
        // -pidfile: write QEMU's process id to this file.
        "-pidfile".into(),
        cfg.pid_file.as_os_str().into(),
    ];

    // In install mode, also attach the ISO as a CD-ROM and boot from it (boot order "d" = CD).
    if install_mode {
        let iso = iso.ok_or("install mode requires an ISO path")?;
        args.push("-cdrom".into());
        args.push(iso.as_os_str().into());
        args.push("-boot".into());
        args.push("d".into());
    }

    Ok(args)
}

// SECTION: CONFIG AND FOLDERS
/// Run init() first if no config file exists yet.
fn ensure_initialized() -> Result<(), String> {
    if !config_path()?.exists() {
        init()?;
    }
    Ok(())
}

/// Build the default settings with paths under the data and run folders.
fn default_config() -> Result<VmConfig, String> {
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

/// Turn settings into the plain "key=value" text saved in the config file.
fn render_config(cfg: &VmConfig) -> String {
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

/// Read the config file (key=value lines) on top of the defaults; blank lines, comments and unknown keys are ignored.
fn load_config() -> Result<VmConfig, String> {
    // Start from defaults so missing keys keep a sensible value.
    let mut cfg = default_config()?;
    let path = config_path()?;
    let text = fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Split each line at the first "="; lines without one are skipped.
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        // Copy each known key into the settings; path keys may start with "~/" which is expanded.
        match key {
            "name" => cfg.name = value.into(),
            "ram" => cfg.ram = value.into(),
            "cpus" => cfg.cpus = value.into(),
            "disk_size" => cfg.disk_size = value.into(),
            "disk_path" => cfg.disk_path = expand_home(value),
            "qmp_socket" => cfg.qmp_socket = expand_home(value),
            "pid_file" => cfg.pid_file = expand_home(value),
            "log_file" => cfg.log_file = expand_home(value),
            _ => {}
        }
    }
    Ok(cfg)
}

/// Full path of the config file.
fn config_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("config"))
}

/// Folder holding the config file.
fn config_dir() -> Result<PathBuf, String> {
    Ok(base_config_dir()?.join(APP_NAME))
}

/// Folder for the disk image and log (under the user's data folder).
fn data_dir() -> Result<PathBuf, String> {
    Ok(base_data_dir()?.join(APP_NAME))
}

/// Folder for runtime files such as the socket and pid file: uses XDG_RUNTIME_DIR, falling back to /tmp.
fn run_dir() -> Result<PathBuf, String> {
    let base = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    Ok(base.join(APP_NAME))
}

/// The user's config base folder: XDG_CONFIG_HOME if set, otherwise .config in the home folder.
fn base_config_dir() -> Result<PathBuf, String> {
    Ok(env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or(home_dir()?.join(".config")))
}

/// The user's data base folder: XDG_DATA_HOME if set, otherwise .local/share in the home folder.
fn base_data_dir() -> Result<PathBuf, String> {
    Ok(env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or(home_dir()?.join(".local/share")))
}

/// The home folder from the HOME variable, or an error if it is not set.
fn home_dir() -> Result<PathBuf, String> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set".into())
}

/// Replace a leading "~/" with the home folder; otherwise use the text as given.
fn expand_home(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        if let Ok(home) = home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(value)
}

// SECTION: SMALL HELPERS
/// Return "found" if the named program is on PATH (checked with the shell's "command -v"), otherwise "missing".
fn found(bin: &str) -> &'static str {
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

/// Check /proc/cpuinfo for the Intel (vmx) or AMD (svm) virtualization flag; returns "unknown" if the file cannot be read.
fn cpu_has_vmx_or_svm() -> &'static str {
    match fs::read_to_string("/proc/cpuinfo") {
        Ok(text) if text.contains(" vmx ") || text.contains(" svm ") => "present",
        Ok(_) => "missing",
        Err(_) => "unknown",
    }
}

/// Read a text file and trim whitespace; None if it cannot be read.
fn read_trimmed(path: &str) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// Read up to 4096 bytes from the socket and return them as text; empty text on error.
fn read_some(stream: &mut UnixStream) -> String {
    let mut buf = [0_u8; 4096];
    match stream.read(&mut buf) {
        Ok(n) => String::from_utf8_lossy(&buf[..n]).to_string(),
        Err(_) => String::new(),
    }
}

/// Join arguments into one printable shell command line, quoting each as needed.
fn shell_join(args: &[OsString]) -> String {
    args.iter()
        .map(|a| shell_quote(&a.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Leave simple text alone; wrap anything with unusual characters in single quotes (escaping inner single quotes).
fn shell_quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=,+".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}
