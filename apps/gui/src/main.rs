//! LiteDroid graphical control panel.

use eframe::egui;
use litedroid_core::DAEMON_SOCKET_PATH;
use serde_json::Value;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const IPC_SOCK: &str = DAEMON_SOCKET_PATH;

#[derive(Clone, Debug)]
struct DeviceRow {
    name: String,
    android_version: String,
    ram_mb: u64,
    vcpu_count: u64,
}

struct LiteDroidApp {
    devices: Vec<DeviceRow>,
    selected_device: String,
    daemon_online: bool,
    power_state: String,
    stats: String,
    diagnostics: Vec<String>,
    status_message: String,
    last_refresh: Instant,
    rendering_mode: String,
    auto_start_attempted: bool,
}

impl Default for LiteDroidApp {
    fn default() -> Self {
        Self::ensure_daemon();
        let mut app = Self {
            devices: Vec::new(),
            selected_device: "default".to_string(),
            daemon_online: false,
            power_state: "offline".to_string(),
            stats: "No VM metrics available".to_string(),
            diagnostics: Vec::new(),
            status_message: "Ready".to_string(),
            last_refresh: Instant::now() - Duration::from_secs(10),
            rendering_mode: rendering_mode(),
            auto_start_attempted: false,
        };
        app.refresh();
        app
    }
}

impl LiteDroidApp {
    fn ensure_daemon() {
        if litedroid_ipc::IpcClient::connect(IPC_SOCK).is_ok() {
            return;
        }
        let Ok(executable) = std::env::current_exe() else {
            return;
        };
        let Some(directory) = executable.parent() else {
            return;
        };
        let daemon = directory.join("litedroid-daemon");
        if !daemon.is_file() {
            return;
        }
        let _ = Command::new(daemon)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }

    fn ipc_call(&mut self, method: &str, params: Value) -> Option<Value> {
        let mut client = match litedroid_ipc::IpcClient::connect(IPC_SOCK) {
            Ok(client) => client,
            Err(error) => {
                self.daemon_online = false;
                self.status_message = format!("Daemon unavailable: {error}");
                return None;
            }
        };
        self.daemon_online = true;
        match client.send_request(method, params) {
            Ok(response) if response.success => response.result,
            Ok(response) => {
                self.status_message = response
                    .error
                    .unwrap_or_else(|| "Request failed".to_string());
                None
            }
            Err(error) => {
                self.status_message = error.to_string();
                None
            }
        }
    }

    fn refresh(&mut self) {
        let config = litedroid_config::LiteDroidConfig::load().unwrap_or_default();
        let _ = config.ensure_default_device();
        self.devices.clear();
        if let Ok(entries) = std::fs::read_dir(config.devices_dir()) {
            for entry in entries.flatten() {
                let metadata_path = entry.path().join("metadata.json");
                let Ok(content) = std::fs::read_to_string(metadata_path) else {
                    continue;
                };
                let Ok(metadata) = serde_json::from_str::<Value>(&content) else {
                    continue;
                };
                self.devices.push(DeviceRow {
                    name: metadata
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("Unknown")
                        .to_string(),
                    android_version: metadata
                        .get("android_version")
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                        .to_string(),
                    ram_mb: metadata.get("ram_mb").and_then(Value::as_u64).unwrap_or(0),
                    vcpu_count: metadata
                        .get("vcpu_count")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                });
            }
        }
        if !self
            .devices
            .iter()
            .any(|device| device.name == self.selected_device)
        {
            if let Some(device) = self.devices.first() {
                self.selected_device = device.name.clone();
            }
        }
        if let Some(status) = self.ipc_call("device.status", serde_json::json!({})) {
            self.power_state = status
                .get("power_state")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
        }
        if self.daemon_online && !self.devices.is_empty() && !self.auto_start_attempted {
            self.auto_start_attempted = true;
            if self
                .ipc_call(
                    "device.start",
                    serde_json::json!({"device": self.selected_device.clone()}),
                )
                .is_some()
            {
                self.status_message = "Starting Android emulator".to_string();
            }
        }
        self.last_refresh = Instant::now();
    }

    fn run_diagnostics(&mut self) {
        self.diagnostics = litedroid_diagnostics::run_diagnostics()
            .iter()
            .map(ToString::to_string)
            .collect();
        self.status_message = "Diagnostics complete".to_string();
    }

    fn create_device(&mut self) {
        let name = format!("PixelLite-{}", self.devices.len() + 1);
        let config = litedroid_config::LiteDroidConfig::load().unwrap_or_default();
        let directory = config.devices_dir().join(&name);
        let device = litedroid_core::DeviceConfig::default();
        let metadata = serde_json::json!({
            "name": name,
            "id": device.id.to_string(),
            "android_version": device.android_version,
            "ram_mb": device.ram_mb,
            "vcpu_count": device.vcpu_count,
        });
        match std::fs::create_dir_all(&directory).and_then(|_| {
            std::fs::write(
                directory.join("metadata.json"),
                serde_json::to_string_pretty(&metadata).unwrap(),
            )
        }) {
            Ok(()) => {
                self.status_message = format!("Created {name}");
                self.refresh();
            }
            Err(error) => self.status_message = format!("Create failed: {error}"),
        }
    }
}

impl eframe::App for LiteDroidApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.last_refresh.elapsed() > Duration::from_secs(3) {
            self.refresh();
        }
        ctx.request_repaint_after(Duration::from_millis(250));

        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("LiteDroid");
                ui.label("Android virtualization control panel");
                ui.separator();
                let (text, color) = if self.daemon_online {
                    ("Daemon online", egui::Color32::from_rgb(85, 190, 120))
                } else {
                    ("Daemon offline", egui::Color32::from_rgb(220, 120, 90))
                };
                ui.colored_label(color, text);
                ui.separator();
                ui.small(format!("Renderer: {}", self.rendering_mode));
            });
        });

        egui::SidePanel::left("devices")
            .resizable(true)
            .default_width(245.0)
            .show(ctx, |ui| {
                ui.heading("Devices");
                ui.add_space(6.0);
                if self.devices.is_empty() {
                    ui.weak("No device profiles found.");
                }
                for device in &self.devices {
                    let selected = self.selected_device == device.name;
                    if ui.selectable_label(selected, &device.name).clicked() {
                        self.selected_device = device.name.clone();
                    }
                    ui.small(format!(
                        "Android {}  |  {} MB  |  {} vCPU",
                        device.android_version, device.ram_mb, device.vcpu_count
                    ));
                    ui.add_space(5.0);
                }
                ui.separator();
                if ui.button("Refresh devices").clicked() {
                    self.refresh();
                }
                if ui.button("Create device profile").clicked() {
                    self.create_device();
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading(format!("{} device", self.selected_device));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Power state:");
                ui.colored_label(egui::Color32::LIGHT_BLUE, &self.power_state);
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(self.daemon_online, egui::Button::new("Start device"))
                    .clicked()
                {
                    if self
                        .ipc_call(
                            "device.start",
                            serde_json::json!({"device": self.selected_device.clone()}),
                        )
                        .is_some()
                    {
                        self.status_message = "Android VM start requested".to_string();
                    }
                }
                if ui
                    .add_enabled(self.daemon_online, egui::Button::new("Stop device"))
                    .clicked()
                {
                    if self
                        .ipc_call("device.stop", serde_json::json!({}))
                        .is_some()
                    {
                        self.status_message = "Android VM stop requested".to_string();
                    }
                }
                if ui.button("Run diagnostics").clicked() {
                    self.run_diagnostics();
                }
            });
            ui.separator();
            ui.heading("VM metrics");
            if ui.button("Refresh metrics").clicked() {
                if let Some(stats) = self.ipc_call("vm.stats", serde_json::json!({})) {
                    self.stats = serde_json::to_string_pretty(&stats).unwrap_or_default();
                }
            }
            egui::ScrollArea::vertical()
                .max_height(170.0)
                .show(ui, |ui| {
                    ui.monospace(&self.stats);
                });
            ui.separator();
            ui.heading("Diagnostics");
            egui::ScrollArea::vertical().show(ui, |ui| {
                if self.diagnostics.is_empty() {
                    ui.weak("Run diagnostics to inspect this host.");
                }
                for result in &self.diagnostics {
                    ui.label(result);
                }
            });
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status_message);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.small(format!("IPC: {IPC_SOCK}"));
                });
            });
        });
    }
}

fn main() -> eframe::Result<()> {
    configure_rendering();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1050.0, 700.0])
            .with_min_inner_size([760.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native(
        "LiteDroid",
        options,
        Box::new(|_creation_context| Ok(Box::new(LiteDroidApp::default()))),
    )
}

fn rendering_mode() -> String {
    if std::env::var("LIBGL_ALWAYS_SOFTWARE").as_deref() == Ok("1") {
        "CPU software".to_string()
    } else {
        "Hardware/automatic".to_string()
    }
}

fn configure_rendering() {
    if std::env::var_os("LIBGL_ALWAYS_SOFTWARE").is_some() {
        return;
    }

    #[cfg(target_os = "linux")]
    if !std::path::Path::new("/dev/dri").exists() {
        std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
    }
}
