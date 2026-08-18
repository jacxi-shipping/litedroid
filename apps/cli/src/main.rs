//! LiteDroid CLI — command-line interface for the Android emulator

use clap::Parser;
use litedroid_core::*;
use litedroid_diagnostics::{run_diagnostics, DiagnosticStatus};
use litedroid_ipc::IpcClient;
use serde_json::json;

const IPC_SOCK: &str = DAEMON_SOCKET_PATH;

#[derive(Parser)]
#[command(name = "litedroid", version, about = "LiteDroid Android Emulator CLI")]
enum Cli {
    Doctor,
    Version,
    Config,
    Devices,
    Device {
        #[command(subcommand)]
        action: DeviceAction,
    },
    Start {
        #[arg(long, default_value = "default")]
        device: String,
        #[arg(long)]
        cold: bool,
        #[arg(long)]
        fast: bool,
    },
    Stop,
    Wipe,
    Restart,
    Pause,
    Resume,
    Status,
    Stats,
    Apk {
        #[command(subcommand)]
        action: ApkAction,
    },
    Launch {
        package: String,
    },
    Shell,
    Logcat,
    Screenshot {
        path: Option<String>,
    },
    Snapshot {
        #[command(subcommand)]
        action: SnapshotAction,
    },
    Adb {
        #[command(subcommand)]
        action: AdbAction,
    },
    Logs {
        #[arg(long)]
        follow: bool,
    },
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
}

#[derive(clap::Subcommand)]
enum DeviceAction {
    Create {
        name: String,
        #[arg(long)]
        resolution: Option<String>,
        #[arg(long)]
        dpi: Option<u32>,
        #[arg(long)]
        ram: Option<u64>,
        #[arg(long)]
        cpu: Option<u32>,
    },
    Delete {
        name: String,
    },
}

#[derive(clap::Subcommand)]
enum ApkAction {
    Install { path: String },
    Uninstall { package: String },
    List,
}

#[derive(clap::Subcommand)]
enum SnapshotAction {
    Create { name: String },
    Restore { name: String },
    List,
    Delete { name: String },
}

#[derive(clap::Subcommand)]
enum AdbAction {
    Shell,
    Install { path: String },
}

#[derive(clap::Subcommand)]
enum DaemonAction {
    Start,
    Stop,
    Status,
}

fn ipc(method: &str, params: serde_json::Value) -> Option<litedroid_ipc::IpcResponse> {
    let mut client = IpcClient::connect(IPC_SOCK).ok()?;
    client.send_request(method, params).ok()
}

fn print_resp(resp: &litedroid_ipc::IpcResponse) -> i32 {
    if resp.success {
        if let Some(ref r) = resp.result {
            println!("{}", serde_json::to_string_pretty(r).unwrap_or_default());
        } else {
            println!("OK");
        }
        0
    } else {
        eprintln!("Error: {}", resp.error.as_deref().unwrap_or("unknown"));
        1
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("litedroid=info".parse().unwrap()),
        )
        .with_target(false)
        .init();

    let code = match Cli::parse() {
        Cli::Doctor => cmd_doctor(),
        Cli::Version => cmd_version(),
        Cli::Config => cmd_config(),
        Cli::Devices => cmd_devices(),
        Cli::Device { action } => cmd_device(action),
        Cli::Start { device, cold, fast } => cmd_start(device, cold, fast),
        Cli::Stop => cmd_stop(),
        Cli::Wipe => cmd_wipe(),
        Cli::Restart => cmd_restart(),
        Cli::Pause => cmd_pause(),
        Cli::Resume => cmd_resume(),
        Cli::Status => cmd_status(),
        Cli::Stats => cmd_stats(),
        Cli::Apk { action } => cmd_apk(action),
        Cli::Launch { package } => cmd_launch(package),
        Cli::Shell => cmd_shell(),
        Cli::Logcat => cmd_logcat(),
        Cli::Screenshot { path } => cmd_screenshot(path),
        Cli::Snapshot { action } => cmd_snapshot(action),
        Cli::Adb { action } => cmd_adb(action),
        Cli::Logs { follow: _ } => cmd_logs(),
        Cli::Daemon { action } => cmd_daemon(action),
    };
    std::process::exit(code);
}

fn cmd_doctor() -> i32 {
    println!("LiteDroid System Diagnostics\n===========================\n");
    let results = run_diagnostics();
    let (mut errs, mut warns) = (0u32, 0u32);
    for r in &results {
        println!("{r}");
        match r.status {
            DiagnosticStatus::Error => errs += 1,
            DiagnosticStatus::Warn => warns += 1,
            _ => {}
        }
    }
    println!();
    if errs > 0 {
        println!("Result: {errs} error(s), {warns} warning(s)");
        1
    } else if warns > 0 {
        println!("Result: {warns} warning(s)");
        0
    } else {
        println!("Result: all checks passed");
        0
    }
}

fn cmd_version() -> i32 {
    println!("litedroid {}", env!("CARGO_PKG_VERSION"));
    println!("IPC protocol version: {}", IPC_PROTOCOL_VERSION);
    println!("Socket: {}", DAEMON_SOCKET_PATH);
    println!("Data dir: {}", DEFAULT_DATA_DIR);
    0
}

fn cmd_config() -> i32 {
    match litedroid_config::LiteDroidConfig::load() {
        Ok(cfg) => {
            println!(
                "{}",
                toml::to_string_pretty(&cfg.global).unwrap_or_default()
            );
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn cmd_devices() -> i32 {
    let cfg = match litedroid_config::LiteDroidConfig::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let dir = cfg.devices_dir();
    if !dir.exists() {
        println!("No devices. Create one: litedroid device create <name>");
        return 0;
    }
    let mut found = false;
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let mp = entry.path().join("metadata.json");
        if let Ok(content) = std::fs::read_to_string(&mp) {
            if let Ok(m) = serde_json::from_str::<serde_json::Value>(&content) {
                println!(
                    "  {}",
                    m.get("name").and_then(|v| v.as_str()).unwrap_or("?")
                );
                println!(
                    "    Android: {}",
                    m.get("android_version")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?")
                );
                println!(
                    "    RAM: {} MB, CPUs: {}",
                    m.get("ram_mb").and_then(|v| v.as_u64()).unwrap_or(0),
                    m.get("vcpu_count").and_then(|v| v.as_u64()).unwrap_or(0)
                );
                println!();
                found = true;
            }
        }
    }
    if !found {
        println!("No devices. Create one: litedroid device create <name>");
    }
    0
}

fn cmd_device(action: DeviceAction) -> i32 {
    match action {
        DeviceAction::Create {
            name,
            resolution,
            dpi,
            ram,
            cpu,
        } => {
            let mut cfg = DeviceConfig::default();
            cfg.name = name.clone();
            if let Some(ref r) = resolution {
                if let Some((w, h)) = r.split_once('x') {
                    if let (Ok(w), Ok(h)) = (w.parse(), h.parse()) {
                        cfg.display.width = w;
                        cfg.display.height = h;
                    }
                }
            }
            if let Some(d) = dpi {
                cfg.display.dpi = d;
            }
            if let Some(r) = ram {
                cfg.ram_mb = r;
            }
            if let Some(c) = cpu {
                cfg.vcpu_count = c;
            }
            let lcfg = match litedroid_config::LiteDroidConfig::load() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("{e}");
                    return 1;
                }
            };
            let dir = lcfg.devices_dir().join(&name);
            std::fs::create_dir_all(&dir).ok();
            let meta = json!({
                "name": cfg.name,
                "id": cfg.id.to_string(),
                "android_version": cfg.android_version,
                "api_level": cfg.api_level,
                "kernel_path": lcfg.images_dir().join("kernel"),
                "initramfs_path": lcfg.images_dir().join("ramdisk.img"),
                "system_image_path": lcfg.images_dir().join("system.img"),
                "profile": format!("{:?}", cfg.profile),
                "ram_mb": cfg.ram_mb,
                "vcpu_count": cfg.vcpu_count,
                "width": cfg.display.width,
                "height": cfg.display.height,
                "dpi": cfg.display.dpi
            });
            match std::fs::write(
                dir.join("metadata.json"),
                serde_json::to_string_pretty(&meta).unwrap(),
            ) {
                Ok(()) => {
                    println!("Device \"{name}\" created");
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            }
        }
        DeviceAction::Delete { name } => {
            let lcfg = match litedroid_config::LiteDroidConfig::load() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("{e}");
                    return 1;
                }
            };
            let dir = lcfg.devices_dir().join(&name);
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => {
                    println!("Deleted \"{name}\"");
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            }
        }
    }
}

fn cmd_start(device: String, cold: bool, fast: bool) -> i32 {
    let mut p = json!({"device": device});
    if cold {
        p["boot_mode"] = json!("cold");
    }
    if fast {
        p["boot_mode"] = json!("fast");
    }
    match ipc("device.start", p) {
        Some(r) => print_resp(&r),
        None => 1,
    }
}
fn cmd_stop() -> i32 {
    match ipc("device.stop", json!({})) {
        Some(r) => print_resp(&r),
        None => 1,
    }
}
fn cmd_wipe() -> i32 {
    match ipc("device.wipe", json!({})) {
        Some(r) => print_resp(&r),
        None => 1,
    }
}
fn cmd_restart() -> i32 {
    match ipc("device.restart", json!({})) {
        Some(r) => print_resp(&r),
        None => 1,
    }
}
fn cmd_pause() -> i32 {
    match ipc("device.pause", json!({})) {
        Some(r) => print_resp(&r),
        None => 1,
    }
}
fn cmd_resume() -> i32 {
    match ipc("device.resume", json!({})) {
        Some(r) => print_resp(&r),
        None => 1,
    }
}
fn cmd_status() -> i32 {
    match ipc("device.status", json!({})) {
        Some(r) => print_resp(&r),
        None => 1,
    }
}
fn cmd_stats() -> i32 {
    match ipc("vm.stats", json!({})) {
        Some(r) => print_resp(&r),
        None => 1,
    }
}

fn cmd_apk(action: ApkAction) -> i32 {
    match action {
        ApkAction::Install { path } => match ipc("apk.install", json!({"path": path})) {
            Some(r) => print_resp(&r),
            None => 1,
        },
        ApkAction::Uninstall { package } => match ipc("apk.uninstall", json!({"package": package}))
        {
            Some(r) => print_resp(&r),
            None => 1,
        },
        ApkAction::List => match ipc("apk.list", json!({})) {
            Some(r) => print_resp(&r),
            None => 1,
        },
    }
}
fn cmd_launch(package: String) -> i32 {
    match ipc("device.launch", json!({"package": package})) {
        Some(r) => print_resp(&r),
        None => 1,
    }
}
fn cmd_shell() -> i32 {
    match ipc("adb.shell", json!({})) {
        Some(r) => print_resp(&r),
        None => 1,
    }
}
fn cmd_logcat() -> i32 {
    match ipc("adb.logcat", json!({})) {
        Some(r) => print_resp(&r),
        None => 1,
    }
}
fn cmd_screenshot(path: Option<String>) -> i32 {
    match ipc("device.screenshot", json!({"path": path})) {
        Some(r) => print_resp(&r),
        None => 1,
    }
}

fn cmd_snapshot(action: SnapshotAction) -> i32 {
    let (method, name) = match action {
        SnapshotAction::Create { name } => ("snapshot.create", Some(name)),
        SnapshotAction::Restore { name } => ("snapshot.restore", Some(name)),
        SnapshotAction::List => ("snapshot.list", None),
        SnapshotAction::Delete { name } => ("snapshot.delete", Some(name)),
    };
    let mut p = json!({});
    if let Some(n) = name {
        p["name"] = json!(n);
    }
    match ipc(method, p) {
        Some(r) => print_resp(&r),
        None => 1,
    }
}

fn cmd_adb(action: AdbAction) -> i32 {
    match action {
        AdbAction::Shell => match ipc("adb.shell", json!({})) {
            Some(r) => print_resp(&r),
            None => 1,
        },
        AdbAction::Install { path } => match ipc("adb.install", json!({"path": path})) {
            Some(r) => print_resp(&r),
            None => 1,
        },
    }
}

fn cmd_logs() -> i32 {
    let cfg = match litedroid_config::LiteDroidConfig::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let log_dir = cfg.data_dir().join("logs");
    if !log_dir.exists() {
        println!("No logs found.");
        return 0;
    }
    for entry in std::fs::read_dir(&log_dir).into_iter().flatten().flatten() {
        if let Ok(content) = std::fs::read_to_string(entry.path()) {
            println!("--- {} ---", entry.path().display());
            for line in content.lines().take(50) {
                println!("{line}");
            }
        }
    }
    0
}

fn cmd_daemon(action: DaemonAction) -> i32 {
    match action {
        DaemonAction::Start => match std::process::Command::new("litedroid-daemon").spawn() {
            Ok(child) => {
                println!("Daemon started (PID: {})", child.id());
                0
            }
            Err(e) => {
                eprintln!("Failed to start daemon: {e}");
                1
            }
        },
        DaemonAction::Stop => match ipc("daemon.shutdown", json!({})) {
            Some(r) => print_resp(&r),
            None => 1,
        },
        DaemonAction::Status => match ipc("daemon.ping", json!({})) {
            Some(r) => print_resp(&r),
            None => 1,
        },
    }
}
