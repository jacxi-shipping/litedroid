//! LiteDroid Daemon — manages VM lifecycle via IPC

use clap::Parser;
use litedroid_config::LiteDroidConfig;
use litedroid_core::*;
use litedroid_ipc::{IpcRequest, IpcResponse, IpcServer, IpcStream};
use parking_lot::Mutex;
use serde_json::json;
use std::io::Write as _;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

#[derive(Parser)]
#[command(name = "litedroid-daemon", about = "LiteDroid daemon process")]
struct Args {
    #[arg(long, default_value = DAEMON_SOCKET_PATH)]
    socket_path: String,
}

struct DaemonState {
    active_device: Option<String>,
    power_state: String,
    stop_handle: Option<Arc<AtomicBool>>,
    emulator: Option<Child>,
    adb: Option<std::path::PathBuf>,
}

fn handle_request(stream: &mut IpcStream, state: &Arc<Mutex<DaemonState>>, _pid: u32) {
    match stream.recv_request() {
        Ok(req) => {
            let resp = dispatch(&req, state);
            let _ = stream.send_response(&resp);
        }
        Err(e) => {
            tracing::warn!("Recv error: {e}");
        }
    }
}

fn dispatch(req: &IpcRequest, state: &Arc<Mutex<DaemonState>>) -> IpcResponse {
    let mut st = state.lock();
    let rid = req.request_id.clone();
    let make_ok = |result: serde_json::Value| IpcResponse {
        version: IPC_PROTOCOL_VERSION,
        request_id: rid.clone(),
        success: true,
        result: Some(result),
        error: None,
    };
    let make_err = |msg: &str| IpcResponse {
        version: IPC_PROTOCOL_VERSION,
        request_id: rid.clone(),
        success: false,
        result: None,
        error: Some(msg.to_string()),
    };

    match req.method.as_str() {
        "daemon.ping" => make_ok(json!({"pong": true, "pid": std::process::id()})),
        "daemon.status" => make_ok(json!({"running": true, "pid": std::process::id()})),
        "daemon.shutdown" => {
            if let Some(mut emulator) = st.emulator.take() {
                let _ = emulator.kill();
                let _ = emulator.wait();
            }
            st.active_device = None;
            st.power_state = "stopping".to_string();
            RUNNING.store(false, Ordering::SeqCst);
            make_ok(json!({"shutting_down": true}))
        }
        "device.start" => {
            let device_name = req
                .params
                .get("device")
                .and_then(|v| v.as_str())
                .unwrap_or("default");
            if let Some(emulator) = st.emulator.as_mut() {
                if emulator.try_wait().ok().flatten().is_none() {
                    return make_err("Android emulator is already running");
                }
                st.emulator = None;
            }
            match launch_device(&mut st, device_name, false) {
                Ok(result) => make_ok(result),
                Err(error) => make_err(&error),
            }
        }
        "device.stop" => {
            if let Some(mut emulator) = st.emulator.take() {
                let _ = emulator.kill();
                let _ = emulator.wait();
            }
            st.adb = None;
            if let Some(stop_handle) = st.stop_handle.take() {
                stop_handle.store(false, Ordering::SeqCst);
            }
            st.active_device = None;
            st.power_state = "off".to_string();
            make_ok(json!({"status": "stopped"}))
        }
        "device.restart" => {
            let device_name = st.active_device.clone().unwrap_or_else(|| "default".to_string());
            stop_emulator(&mut st);
            match launch_device(&mut st, &device_name, false) {
                Ok(result) => make_ok(result),
                Err(error) => make_err(&error),
            }
        }
        "device.wipe" => {
            let device_name = st.active_device.clone().unwrap_or_else(|| "default".to_string());
            stop_emulator(&mut st);
            match launch_device(&mut st, &device_name, true) {
                Ok(result) => make_ok(result),
                Err(error) => make_err(&error),
            }
        }
        "device.pause" => make_err("Pause is not implemented for the active VM"),
        "device.resume" => make_err("Resume is not implemented for the active VM"),
        "device.status" => make_ok(json!({
            "power_state": emulator_state(&mut st),
            "device": st.active_device,
        })),
        "device.launch" => {
            let package = match req.params.get("package").and_then(|value| value.as_str()) {
                Some(package) => package,
                None => return make_err("A package name is required"),
            };
            match adb_output(&mut st, ["shell", "monkey", "-p", package, "-c", "android.intent.category.LAUNCHER", "1"]) {
                Ok(output) => make_ok(json!({"package": package, "output": output})),
                Err(error) => make_err(&error),
            }
        }
        "device.screenshot" => {
            let path = req
                .params
                .get("path")
                .and_then(|value| value.as_str())
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    LiteDroidConfig::load()
                        .map(|config| config.data_dir().join("screenshots").join("device.png"))
                        .unwrap_or_else(|_| std::path::PathBuf::from("device.png"))
                });
            if let Some(parent) = path.parent() {
                if let Err(error) = std::fs::create_dir_all(parent) {
                    return make_err(&format!("Cannot create screenshot directory: {error}"));
                }
            }
            match adb_bytes(&mut st, ["exec-out", "screencap", "-p"]) {
                Ok(bytes) => match std::fs::write(&path, bytes) {
                    Ok(()) => make_ok(json!({"path": path})),
                    Err(error) => make_err(&format!("Cannot save screenshot: {error}")),
                },
                Err(error) => make_err(&error),
            }
        }
        "vm.stats" => make_ok(json!({
            "cpu": {"host_percent": 0.0, "vcpu_percent": [], "guest_time_ms": 0},
            "memory": {"allocated_mb": 0, "used_mb": 0, "host_rss_mb": 0},
            "gpu": {"utilization_percent": 0.0, "fps": 0.0, "memory_mb": 0, "hw_accelerated": false},
            "network": {"rx_bytes_per_sec": 0, "tx_bytes_per_sec": 0, "guest_ip": null},
            "uptime_secs": 0,
        })),
        "apk.install" | "adb.install" => {
            let path = match req.params.get("path").and_then(|value| value.as_str()) {
                Some(path) if std::path::Path::new(path).is_file() => path,
                Some(path) => return make_err(&format!("APK not found: {path}")),
                None => return make_err("An APK path is required"),
            };
            match adb_output(&mut st, ["install", "-r", path]) {
                Ok(output) => make_ok(json!({"output": output})),
                Err(error) => make_err(&error),
            }
        }
        "apk.uninstall" => {
            let package = match req.params.get("package").and_then(|value| value.as_str()) {
                Some(package) => package,
                None => return make_err("A package name is required"),
            };
            match adb_output(&mut st, ["uninstall", package]) {
                Ok(output) => make_ok(json!({"output": output})),
                Err(error) => make_err(&error),
            }
        }
        "apk.list" => match adb_output(&mut st, ["shell", "pm", "list", "packages", "-3"]) {
            Ok(output) => make_ok(json!({"packages": output.lines().collect::<Vec<_>>() })),
            Err(error) => make_err(&error),
        },
        "adb.shell" => match adb_output(&mut st, ["shell"]) {
            Ok(output) => make_ok(json!({"output": output})),
            Err(error) => make_err(&error),
        },
        "adb.logcat" => match adb_output(&mut st, ["logcat", "-d", "-t", "200"]) {
            Ok(output) => make_ok(json!({"output": output})),
            Err(error) => make_err(&error),
        },
        "snapshot.create" => make_err("No active VM"),
        "snapshot.restore" => make_err("No active VM"),
        "snapshot.list" => make_err("No active VM"),
        "snapshot.delete" => make_err("No active VM"),
        _ => make_err(&format!("Unknown method: {}", req.method)),
    }
}

fn stop_emulator(state: &mut DaemonState) {
    if let Some(mut emulator) = state.emulator.take() {
        let _ = emulator.kill();
        let _ = emulator.wait();
    }
    state.adb = None;
    state.active_device = None;
    state.power_state = "off".to_string();
}

fn launch_device(
    state: &mut DaemonState,
    device_name: &str,
    wipe_data: bool,
) -> std::result::Result<serde_json::Value, String> {
    let config = LiteDroidConfig::load().map_err(|error| format!("Cannot load configuration: {error}"))?;
    let device = match config.device_config(device_name) {
        Ok(config) => config,
        Err(_) if device_name == "default" => config
            .ensure_default_device()
            .map_err(|error| format!("Cannot create default device: {error}"))?,
        Err(error) => return Err(format!("Cannot load device {device_name}: {error}")),
    };
    config
        .ensure_android_images(device.api_level)
        .map_err(|error| format!("Android images are unavailable: {error}"))?;
    let avd_name = avd_name(device_name, device.api_level);
    let tools = android_emulator(&config, &device, &avd_name)?;
    let emulator_log = config.data_dir().join("logs").join("android-emulator.log");
    let log_file = std::fs::File::create(&emulator_log)
        .map_err(|error| format!("Cannot create emulator log: {error}"))?;
    let log_copy = log_file
        .try_clone()
        .map_err(|error| format!("Cannot prepare emulator log: {error}"))?;
    let mut command = Command::new(&tools.emulator);
    command
        .args([
            "-avd",
            &avd_name,
            "-port",
            "5554",
            "-gpu",
            "swiftshader_indirect",
            "-no-snapshot",
            "-no-metrics",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_copy))
        .stderr(Stdio::from(log_file));
    if wipe_data {
        command.arg("-wipe-data");
    }
    let child = command
        .spawn()
        .map_err(|error| format!("Cannot start Android emulator: {error}"))?;
    state.active_device = Some(device_name.to_string());
    state.power_state = "booting".to_string();
    state.emulator = Some(child);
    state.adb = Some(tools.adb);
    Ok(json!({
        "device": device_name,
        "status": "booting",
        "avd": avd_name,
        "log": emulator_log,
        "wipe_data": wipe_data,
    }))
}

struct AndroidTools {
    emulator: std::path::PathBuf,
    adb: std::path::PathBuf,
}

fn adb_bytes<I, S>(state: &mut DaemonState, args: I) -> std::result::Result<Vec<u8>, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    if emulator_state(state) != "running" {
        return Err("Android is still booting; wait until device status is running".to_string());
    }
    let adb = state
        .adb
        .as_ref()
        .ok_or_else(|| "ADB is unavailable because no device is running".to_string())?;
    let output = Command::new(adb)
        .arg("-s")
        .arg("emulator-5554")
        .args(args)
        .output()
        .map_err(|error| format!("Cannot run ADB: {error}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(format!(
            "ADB command failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn adb_output<I, S>(state: &mut DaemonState, args: I) -> std::result::Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    adb_bytes(state, args).map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string())
}

fn avd_name(device_name: &str, api_level: u32) -> String {
    let safe_name: String = device_name
        .chars()
        .map(|character| if character.is_ascii_alphanumeric() { character } else { '_' })
        .collect();
    format!("LiteDroid-{safe_name}-API{api_level}")
}

fn android_emulator(
    config: &LiteDroidConfig,
    device: &DeviceConfig,
    avd_name: &str,
) -> std::result::Result<AndroidTools, String> {
    let sdk_root = std::env::var_os("ANDROID_SDK_ROOT")
        .or_else(|| std::env::var_os("ANDROID_HOME"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| config.data_dir().join("android-sdk"));
    let emulator = sdk_root.join("emulator").join("emulator");
    let adb = sdk_root.join("platform-tools").join("adb");
    let avdmanager = sdk_root
        .join("cmdline-tools")
        .join("latest")
        .join("bin")
        .join("avdmanager");
    if !emulator.is_file() || !avdmanager.is_file() || !adb.is_file() {
        return Err(format!(
            "Android Emulator tools are not installed under {}. Run scripts/linux/setup.sh first.",
            sdk_root.display()
        ));
    }
    let avd_dir = dirs::home_dir()
        .unwrap_or_default()
        .join(".android")
        .join("avd")
        .join(format!("{avd_name}.avd"));
    if !avd_dir.is_dir() {
        let mut create = Command::new(avdmanager)
            .args([
                "create",
                "avd",
                "--force",
                "--name",
                avd_name,
                "--package",
                &format!("system-images;android-{};default;x86_64", device.api_level),
                "--device",
                "pixel_5",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Cannot start avdmanager: {error}"))?;
        if let Some(mut stdin) = create.stdin.take() {
            stdin
                .write_all(b"no\n")
                .map_err(|error| format!("Cannot configure Android AVD: {error}"))?;
        }
        let output = create
            .wait_with_output()
            .map_err(|error| format!("Cannot create Android AVD: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "Cannot create Android AVD: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    configure_avd(&avd_dir, device)?;
    Ok(AndroidTools { emulator, adb })
}

fn configure_avd(avd_dir: &std::path::Path, device: &DeviceConfig) -> std::result::Result<(), String> {
    let config_path = avd_dir.join("config.ini");
    let existing = std::fs::read_to_string(&config_path)
        .map_err(|error| format!("Cannot read AVD configuration: {error}"))?;
    let managed_keys = [
        "hw.cpu.ncore=",
        "hw.ramSize=",
        "hw.lcd.width=",
        "hw.lcd.height=",
        "hw.lcd.density=",
    ];
    let mut contents: String = existing
        .lines()
        .filter(|line| !managed_keys.iter().any(|key| line.starts_with(key)))
        .map(|line| format!("{line}\n"))
        .collect();
    contents.push_str(&format!(
        "hw.cpu.ncore={}\nhw.ramSize={}\nhw.lcd.width={}\nhw.lcd.height={}\nhw.lcd.density={}\n",
        device.vcpu_count,
        device.ram_mb,
        device.display.width,
        device.display.height,
        device.display.dpi,
    ));
    std::fs::write(config_path, contents)
        .map_err(|error| format!("Cannot update AVD configuration: {error}"))
}

fn emulator_state(state: &mut DaemonState) -> String {
    if let Some(emulator) = state.emulator.as_mut() {
        match emulator.try_wait() {
            Ok(None) => {
                let adb_state = state
                    .adb
                    .as_ref()
                    .and_then(|adb| {
                        Command::new(adb)
                            .args(["-s", "emulator-5554", "get-state"])
                            .output()
                            .ok()
                    })
                    .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string());
                state.power_state = if adb_state.as_deref() == Some("device") {
                    "running".to_string()
                } else {
                    "booting".to_string()
                };
            }
            Ok(Some(status)) => {
                state.emulator = None;
                state.adb = None;
                state.active_device = None;
                state.power_state = if status.success() {
                    "off".to_string()
                } else {
                    "error".to_string()
                };
            }
            Err(_) => state.power_state = "error".to_string(),
        }
    }
    state.power_state.clone()
}

fn main() {
    let args = Args::parse();
    let socket_path = args.socket_path;

    // Initialize logging
    let cfg = LiteDroidConfig::load().ok();
    let log_dir = cfg
        .as_ref()
        .map(|c| c.data_dir())
        .unwrap_or_default()
        .join("logs");
    let _guard = match litedroid_logging::init("info", &log_dir) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("Failed to init logging: {e}");
            return;
        }
    };

    let pid = std::process::id();
    tracing::info!("LiteDroid daemon starting (PID: {pid})");

    // Write PID file
    let pid_path = DAEMON_PID_PATH;
    if let Err(e) = std::fs::write(pid_path, pid.to_string()) {
        tracing::warn!("Cannot write PID file: {e}");
    }

    // Set up signal handler
    let _running = Arc::new(AtomicBool::new(true));
    let sa_term = nix::sys::signal::SigAction::new(
        nix::sys::signal::SigHandler::Handler(sigterm_handler),
        nix::sys::signal::SaFlags::empty(),
        nix::sys::signal::SigSet::empty(),
    );
    let _ = unsafe { nix::sys::signal::sigaction(nix::sys::signal::Signal::SIGTERM, &sa_term) };
    let sa_int = nix::sys::signal::SigAction::new(
        nix::sys::signal::SigHandler::Handler(sigint_handler),
        nix::sys::signal::SaFlags::empty(),
        nix::sys::signal::SigSet::empty(),
    );
    let _ = unsafe { nix::sys::signal::sigaction(nix::sys::signal::Signal::SIGINT, &sa_int) };

    // Store running flags in statics so signal handlers can access them
    RUNNING.store(true, Ordering::SeqCst);

    // Clean up old socket
    let _ = std::fs::remove_file(&socket_path);

    // Bind IPC server
    let server = match IpcServer::new(&socket_path) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Cannot bind IPC socket at {socket_path}: {e}");
            let _ = std::fs::remove_file(pid_path);
            return;
        }
    };
    tracing::info!("IPC server listening on {socket_path}");

    // Set socket permissions
    #[allow(unused_imports)]
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(&socket_path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o600);
        let _ = std::fs::set_permissions(&socket_path, perms);
    }

    let state = Arc::new(Mutex::new(DaemonState {
        active_device: None,
        power_state: "off".to_string(),
        stop_handle: None,
        emulator: None,
        adb: None,
    }));

    // Accept loop
    while RUNNING.load(Ordering::SeqCst) {
        match server.accept() {
            Ok((mut stream, _addr)) => {
                let st = state.clone();
                thread::spawn(move || {
                    handle_request(&mut stream, &st, pid);
                });
            }
            Err(e) => {
                if RUNNING.load(Ordering::SeqCst) {
                    tracing::debug!("Accept error: {e}");
                }
            }
        }
        // Small sleep to avoid busy-loop when no connections
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    // Cleanup
    let _ = std::fs::remove_file(&socket_path);
    let _ = std::fs::remove_file(pid_path);
    tracing::info!("Daemon stopped");
    litedroid_logging::shutdown();
}

static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn sigterm_handler(_: libc::c_int) {
    RUNNING.store(false, Ordering::SeqCst);
}

extern "C" fn sigint_handler(_: libc::c_int) {
    RUNNING.store(false, Ordering::SeqCst);
}
