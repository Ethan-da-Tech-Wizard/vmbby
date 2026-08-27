use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const APP_NAME: &str = "vmbby";
const DEFAULT_RAM: &str = "6G";
const DEFAULT_CPUS: &str = "4";
const DEFAULT_DISK_SIZE: &str = "60G";

#[derive(Clone, Debug)]
struct VmConfig {
    name: String,
    ram: String,
    cpus: String,
    disk_size: String,
    disk_path: PathBuf,
    qmp_socket: PathBuf,
    pid_file: PathBuf,
    log_file: PathBuf,
}

fn main() {
    if let Err(err) = real_main() {
        eprintln!("vmbby: {err}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<(), String> {
    let mut args = env::args_os();
    let _bin = args.next();
    let command = args
        .next()
        .unwrap_or_else(|| OsString::from("help"))
        .to_string_lossy()
        .to_string();

    match command.as_str() {
        "doctor" => doctor(),
        "init" => init(),
        "install" => {
            let iso = args
                .next()
                .ok_or("usage: vmbby install /path/to/ubuntu.iso")?;
            install(Path::new(&iso))
        }
        "run" => run(false),
        "gui" | "portal" => gui(),
        "stop" => stop(),
        "status" => status(),
        "logs" => logs(),
        "explain" => explain(),
        "command" => print_command(false, None),
        "help" | "--help" | "-h" => {
            help();
            Ok(())
        }
        other => Err(format!("unknown command '{other}'. Try: vmbby help")),
    }
}

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

fn init() -> Result<(), String> {
    let cfg = default_config()?;
    fs::create_dir_all(config_dir()?).map_err(|e| format!("create config dir: {e}"))?;
    fs::create_dir_all(data_dir()?).map_err(|e| format!("create data dir: {e}"))?;
    fs::create_dir_all(run_dir()?).map_err(|e| format!("create run dir: {e}"))?;

    let cfg_path = config_path()?;
    if !cfg_path.exists() {
        fs::write(&cfg_path, render_config(&cfg)).map_err(|e| format!("write config: {e}"))?;
        println!("created {}", cfg_path.display());
    } else {
        println!("config already exists: {}", cfg_path.display());
    }

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

    if !Path::new("/dev/kvm").exists() {
        println!();
        println!("KVM is not visible. On Arch this usually means the kvm_intel module is not loaded or this environment hides /dev/kvm.");
        println!("Useful checks: lsmod | rg '^kvm', sudo modprobe kvm_intel, then confirm your user can access /dev/kvm.");
    }

    Ok(())
}

fn install(iso: &Path) -> Result<(), String> {
    if !iso.exists() {
        return Err(format!("ISO not found: {}", iso.display()));
    }
    ensure_initialized()?;
    launch(true, Some(iso))
}

fn run(_from_gui: bool) -> Result<(), String> {
    ensure_initialized()?;
    launch(false, None)
}

fn launch(install_mode: bool, iso: Option<&Path>) -> Result<(), String> {
    let cfg = load_config()?;
    if cfg.pid_file.exists() {
        println!("pid file exists: {}", cfg.pid_file.display());
        println!("If the VM is not actually running, remove that stale file.");
    }

    let args = qemu_args(&cfg, install_mode, iso)?;
    println!("starting Ubuntu VM:");
    println!("qemu-system-x86_64 {}", shell_join(&args));

    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&cfg.log_file)
        .map_err(|e| format!("open log file: {e}"))?;
    let log_err = log.try_clone().map_err(|e| format!("clone log file: {e}"))?;

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

fn stop() -> Result<(), String> {
    let cfg = load_config()?;
    if !cfg.qmp_socket.exists() {
        return Err(format!("QMP socket not found: {}. VM may not be running.", cfg.qmp_socket.display()));
    }

    let mut stream = UnixStream::connect(&cfg.qmp_socket).map_err(|e| format!("connect QMP: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| format!("set QMP timeout: {e}"))?;
    let _ = read_some(&mut stream);
    stream
        .write_all(b"{\"execute\":\"qmp_capabilities\"}\n")
        .map_err(|e| format!("send qmp_capabilities: {e}"))?;
    let _ = read_some(&mut stream);
    stream
        .write_all(b"{\"execute\":\"system_powerdown\"}\n")
        .map_err(|e| format!("send system_powerdown: {e}"))?;
    println!("sent ACPI shutdown request to Ubuntu");
    Ok(())
}

fn status() -> Result<(), String> {
    let cfg = load_config()?;
    println!("config: {}", config_path()?.display());
    println!("disk: {}", cfg.disk_path.display());
    println!("qmp: {}", cfg.qmp_socket.display());
    println!("pid file: {}", cfg.pid_file.display());
    println!("log: {}", cfg.log_file.display());
    println!("vm state: {}", if cfg.qmp_socket.exists() { "probably running" } else { "not running or no QMP socket" });
    Ok(())
}

fn logs() -> Result<(), String> {
    let cfg = load_config().unwrap_or(default_config()?);
    if !cfg.log_file.exists() {
        println!("no log yet: {}", cfg.log_file.display());
        return Ok(());
    }
    let text = fs::read_to_string(&cfg.log_file)
        .map_err(|e| format!("read {}: {e}", cfg.log_file.display()))?;
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(160);
    for line in &lines[start..] {
        println!("{line}");
    }
    Ok(())
}

fn gui() -> Result<(), String> {
    let helper = gui_helper_path()?;
    if !helper.exists() {
        return Err(format!("GUI helper missing: {}", helper.display()));
    }
    Command::new("python3")
        .arg(helper)
        .spawn()
        .map_err(|e| format!("start GUI helper: {e}"))?;
    Ok(())
}

fn gui_helper_path() -> Result<PathBuf, String> {
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

    Ok(PathBuf::from("/usr/share/vmbby/assets/vmbby_controller.py"))
}

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
  simple and low-risk for school work.

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

fn print_command(install_mode: bool, iso: Option<&Path>) -> Result<(), String> {
    let cfg = load_config().unwrap_or(default_config()?);
    println!("qemu-system-x86_64 {}", shell_join(&qemu_args(&cfg, install_mode, iso)?));
    Ok(())
}

fn qemu_args(cfg: &VmConfig, install_mode: bool, iso: Option<&Path>) -> Result<Vec<OsString>, String> {
    let kvm = Path::new("/dev/kvm").exists();
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
        "-display".into(),
        "gtk,gl=off".into(),
        "-device".into(),
        "virtio-vga".into(),
        "-device".into(),
        "qemu-xhci".into(),
        "-device".into(),
        "usb-tablet".into(),
        "-qmp".into(),
        format!("unix:{},server=on,wait=off", cfg.qmp_socket.display()).into(),
        "-pidfile".into(),
        cfg.pid_file.as_os_str().into(),
    ];

    if install_mode {
        let iso = iso.ok_or("install mode requires an ISO path")?;
        args.push("-cdrom".into());
        args.push(iso.as_os_str().into());
        args.push("-boot".into());
        args.push("d".into());
    }

    Ok(args)
}

fn ensure_initialized() -> Result<(), String> {
    if !config_path()?.exists() {
        init()?;
    }
    Ok(())
}

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

fn load_config() -> Result<VmConfig, String> {
    let mut cfg = default_config()?;
    let path = config_path()?;
    let text = fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
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

fn config_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("config"))
}

fn config_dir() -> Result<PathBuf, String> {
    Ok(base_config_dir()?.join(APP_NAME))
}

fn data_dir() -> Result<PathBuf, String> {
    Ok(base_data_dir()?.join(APP_NAME))
}

fn run_dir() -> Result<PathBuf, String> {
    let base = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    Ok(base.join(APP_NAME))
}

fn base_config_dir() -> Result<PathBuf, String> {
    Ok(env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or(home_dir()?.join(".config")))
}

fn base_data_dir() -> Result<PathBuf, String> {
    Ok(env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or(home_dir()?.join(".local/share")))
}

fn home_dir() -> Result<PathBuf, String> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set".into())
}

fn expand_home(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        if let Ok(home) = home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(value)
}

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

fn cpu_has_vmx_or_svm() -> &'static str {
    match fs::read_to_string("/proc/cpuinfo") {
        Ok(text) if text.contains(" vmx ") || text.contains(" svm ") => "present",
        Ok(_) => "missing",
        Err(_) => "unknown",
    }
}

fn read_trimmed(path: &str) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn read_some(stream: &mut UnixStream) -> String {
    let mut buf = [0_u8; 4096];
    match stream.read(&mut buf) {
        Ok(n) => String::from_utf8_lossy(&buf[..n]).to_string(),
        Err(_) => String::new(),
    }
}

fn shell_join(args: &[OsString]) -> String {
    args.iter()
        .map(|a| shell_quote(&a.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=,+".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}
