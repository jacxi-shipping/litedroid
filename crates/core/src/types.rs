use serde::{Deserialize, Serialize};

/// CPU architecture of the emulated device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Architecture {
    X86_64,
    Aarch64,
}

impl std::fmt::Display for Architecture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Architecture::X86_64 => write!(f, "x86_64"),
            Architecture::Aarch64 => write!(f, "aarch64"),
        }
    }
}

/// Current power state of a virtual device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PowerState {
    Off,
    Starting,
    Running,
    Pausing,
    Paused,
    Resuming,
    Stopping,
    Error,
}

impl std::fmt::Display for PowerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PowerState::Off => write!(f, "off"),
            PowerState::Starting => write!(f, "starting"),
            PowerState::Running => write!(f, "running"),
            PowerState::Pausing => write!(f, "pausing"),
            PowerState::Paused => write!(f, "paused"),
            PowerState::Resuming => write!(f, "resuming"),
            PowerState::Stopping => write!(f, "stopping"),
            PowerState::Error => write!(f, "error"),
        }
    }
}

/// Boot mode: cold boot vs. fast resume from snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BootMode {
    Cold,
    Fast,
}

/// Android ABI (application binary interface) identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AndroidAbi {
    X86_64,
    Arm64V8a,
    ArmeabiV7a,
}

impl std::fmt::Display for AndroidAbi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AndroidAbi::X86_64 => write!(f, "x86_64"),
            AndroidAbi::Arm64V8a => write!(f, "arm64-v8a"),
            AndroidAbi::ArmeabiV7a => write!(f, "armeabi-v7a"),
        }
    }
}

/// Guest display configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayConfig {
    pub width: u32,
    pub height: u32,
    pub dpi: u32,
    pub refresh_rate: u32,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            width: 1080,
            height: 1920,
            dpi: 420,
            refresh_rate: 60,
        }
    }
}

/// Transport-layer protocol for port forwarding rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PortProtocol {
    Tcp,
    Udp,
}

/// A single port-forwarding rule from host to guest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortForward {
    pub host_port: u16,
    pub guest_port: u16,
    pub protocol: PortProtocol,
}

/// Guest network configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub enabled: bool,
    pub port_forwards: Vec<PortForward>,
    pub guest_ip: String,
    pub gateway_ip: String,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port_forwards: Vec::new(),
            guest_ip: "10.0.2.15".to_string(),
            gateway_ip: "10.0.2.2".to_string(),
        }
    }
}

/// Guest audio configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    pub enabled: bool,
    pub backend: String,
    pub sample_rate: u32,
    pub channels: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            backend: "pulseaudio".to_string(),
            sample_rate: 48000,
            channels: 2,
        }
    }
}

/// Guest storage / disk-image configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub userdata_path: String,
    pub userdata_size_mb: u64,
    pub cache_path: String,
    pub cache_size_mb: u64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            userdata_path: String::new(),
            userdata_size_mb: 4096,
            cache_path: String::new(),
            cache_size_mb: 1024,
        }
    }
}

/// Metadata about a saved VM snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub name: String,
    pub created_at: String,
    pub size_bytes: u64,
    pub boot_mode: BootMode,
    pub description: String,
}

/// Parsed information extracted from an APK manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApkInfo {
    pub package_name: String,
    pub version_name: String,
    pub version_code: u32,
    pub min_sdk: u32,
    pub target_sdk: u32,
    pub abis: Vec<AndroidAbi>,
    pub permissions: Vec<String>,
    pub size_bytes: u64,
}

/// A 2-D coordinate (guest pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// Input events forwarded from the host to the guest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputEvent {
    MouseDown { button: MouseButton, x: i32, y: i32 },
    MouseUp { button: MouseButton, x: i32, y: i32 },
    MouseMove { x: i32, y: i32 },
    KeyDown { keycode: u32 },
    KeyUp { keycode: u32 },
    TouchStart { slot: u32, x: i32, y: i32 },
    TouchMove { slot: u32, x: i32, y: i32 },
    TouchEnd { slot: u32 },
    AndroidButton(AndroidButton),
}

/// Mouse button identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Special Android hardware buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AndroidButton {
    Back,
    Home,
    Recent,
    Power,
    VolumeUp,
    VolumeDown,
}
