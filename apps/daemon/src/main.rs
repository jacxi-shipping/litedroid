//! LiteDroid Daemon — manages VM lifecycle via IPC

use clap::Parser;
use litedroid_core::*;
use litedroid_config::LiteDroidConfig;
use litedroid_ipc::{IpcRequest, IpcResponse, IpcServer, IpcStream};
use parking_lot::Mutex;
use serde_json::json;
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
            st.power_state = "stopping".to_string();
            make_ok(json!({"shutting_down": true}))
        }
        "device.start" => {
            let device_name = req.params.get("device").and_then(|v| v.as_str()).unwrap_or("default");
            if st.stop_handle.is_some() {
                return make_err("VM is already running");
            }
            let config = match LiteDroidConfig::load() {
                Ok(config) => config,
                Err(error) => return make_err(&format!("Cannot load configuration: {error}")),
            };
            let vm_config = config.default_device_config();
            if !vm_config.kernel_path.is_file() {
                return make_err(&format!("Kernel image not found: {}", vm_config.kernel_path.display()));
            }
            if !vm_config.initramfs_path.is_file() {
                return make_err(&format!("Initramfs image not found: {}", vm_config.initramfs_path.display()));
            }
            if !vm_config.system_image_path.is_file() {
                return make_err(&format!("Android system image not found: {}", vm_config.system_image_path.display()));
            }
            let mut vm = match litedroid_vmm::VirtualMachine::new(&vm_config) {
                Ok(vm) => vm,
                Err(error) => return make_err(&format!("VM creation failed: {error}")),
            };
            if let Err(error) = vm.load_kernel(&vm_config.kernel_path).and_then(|_| vm.load_initramfs(&vm_config.initramfs_path)).and_then(|_| vm.setup_boot()) {
                return make_err(&format!("Guest boot setup failed: {error}"));
            }
            let stop_handle = vm.stop_handle();
            let thread_state = state.clone();
            thread::spawn(move || {
                let result = vm.run();
                let mut state = thread_state.lock();
                state.stop_handle = None;
                state.active_device = None;
                state.power_state = if result.is_ok() { "off".to_string() } else { "error".to_string() };
                if let Err(error) = result {
                    tracing::error!("VM exited with error: {error}");
                }
            });
            st.active_device = Some(device_name.to_string());
            st.power_state = "running".to_string();
            st.stop_handle = Some(stop_handle);
            make_ok(json!({"device": device_name, "status": "running"}))
        }
        "device.stop" => {
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
            "power_state": st.power_state,
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

fn main() {
    let args = Args::parse();
    let socket_path = args.socket_path;

    // Initialize logging
    let cfg = LiteDroidConfig::load().ok();
    let log_dir = cfg.as_ref().map(|c| c.data_dir()).unwrap_or_default().join("logs");
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
        perms.set_mode(0o666);
        let _ = std::fs::set_permissions(&socket_path, perms);
    }

    let state = Arc::new(Mutex::new(DaemonState {
        active_device: None,
        power_state: "off".to_string(),
        stop_handle: None,
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
