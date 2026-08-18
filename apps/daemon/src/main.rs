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
            let config = match LiteDroidConfig::load() {
                Ok(config) => config,
                Err(error) => return make_err(&format!("Cannot load configuration: {error}")),
            };
            let vm_config = match config.device_config(device_name) {
                Ok(config) => config,
                Err(_) if device_name == "default" => match config.ensure_default_device() {
                    Ok(config) => config,
                    Err(error) => {
                        return make_err(&format!("Cannot create default device: {error}"))
                    }
                },
                Err(error) => {
                    return make_err(&format!("Cannot load device {device_name}: {error}"))
                }
            };
            if let Err(error) = config.ensure_android_images(vm_config.api_level) {
                return make_err(&format!("Android images are unavailable: {error}"));
            }
            let tools = match android_emulator(&config) {
                Ok(tools) => tools,
                Err(error) => return make_err(&error),
            };
            let avd_name = format!("LiteDroid-API{}", vm_config.api_level);
            let emulator_log = config.data_dir().join("logs").join("android-emulator.log");
            let log_file = match std::fs::File::create(&emulator_log) {
                Ok(file) => file,
                Err(error) => return make_err(&format!("Cannot create emulator log: {error}")),
            };
            let log_copy = match log_file.try_clone() {
                Ok(file) => file,
                Err(error) => return make_err(&format!("Cannot prepare emulator log: {error}")),
            };
            let child = match Command::new(tools.emulator)
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
                .stderr(Stdio::from(log_file))
                .spawn()
            {
                Ok(child) => child,
                Err(error) => return make_err(&format!("Cannot start Android emulator: {error}")),
            };
            st.active_device = Some(device_name.to_string());
            st.power_state = "booting".to_string();
            st.emulator = Some(child);
            st.adb = Some(tools.adb);
            make_ok(
                json!({"device": device_name, "status": "booting", "avd": avd_name, "log": emulator_log}),
            )
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
        "device.restart" => make_err("VM not running"),
        "device.pause" => make_err("Pause is not implemented for the active VM"),
        "device.resume" => make_err("Resume is not implemented for the active VM"),
        "device.status" => make_ok(json!({
            "power_state": emulator_state(&mut st),
            "device": st.active_device,
        })),
        "device.launch" => make_err("Device not running"),
        "device.screenshot" => make_err("Device not running"),
        "vm.stats" => make_ok(json!({
            "cpu": {"host_percent": 0.0, "vcpu_percent": [], "guest_time_ms": 0},
            "memory": {"allocated_mb": 0, "used_mb": 0, "host_rss_mb": 0},
            "gpu": {"utilization_percent": 0.0, "fps": 0.0, "memory_mb": 0, "hw_accelerated": false},
            "network": {"rx_bytes_per_sec": 0, "tx_bytes_per_sec": 0, "guest_ip": null},
            "uptime_secs": 0,
        })),
        "apk.install" => make_err("ADB bridge not yet connected"),
        "apk.uninstall" => make_err("ADB bridge not yet connected"),
        "apk.list" => make_err("ADB bridge not yet connected"),
        "adb.shell" => make_err("ADB not yet connected"),
        "adb.install" => make_err("ADB not yet connected"),
        "adb.logcat" => make_err("ADB not yet connected"),
        "snapshot.create" => make_err("No active VM"),
        "snapshot.restore" => make_err("No active VM"),
        "snapshot.list" => make_err("No active VM"),
        "snapshot.delete" => make_err("No active VM"),
        _ => make_err(&format!("Unknown method: {}", req.method)),
    }
}

struct AndroidTools {
    emulator: std::path::PathBuf,
    adb: std::path::PathBuf,
}

fn android_emulator(config: &LiteDroidConfig) -> std::result::Result<AndroidTools, String> {
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
    let avd_name = "LiteDroid-API34";
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
                "system-images;android-34;default;x86_64",
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
    Ok(AndroidTools { emulator, adb })
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
