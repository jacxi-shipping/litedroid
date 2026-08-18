use std::path::PathBuf;

use thiserror::Error;

/// Master error type for the LiteDroid emulator.
#[derive(Debug, Error)]
pub enum LiteDroidError {
    #[error("KVM unavailable: {0}")]
    KvmUnavailable(String),

    #[error("KVM ioctl error: {0}")]
    KvmIoctl(String),

    #[error("VM creation failed: {0}")]
    VmCreationFailed(String),

    #[error("vCPU creation failed (cpu_index={cpu_index}): {reason}")]
    VcpuCreationFailed { cpu_index: u32, reason: String },

    #[error("vCPU run failed (cpu_index={cpu_index}): {reason}")]
    VcpuRunFailed { cpu_index: u32, reason: String },

    #[error("Memory allocation failed: requested {requested_mb}MB, available {available_mb}MB")]
    MemoryAllocationFailed {
        requested_mb: u64,
        available_mb: u64,
    },

    #[error("Memory map failed: {0}")]
    MemoryMapFailed(String),

    #[error("Guest address out of range: {address:#x}")]
    GuestAddressOutOfRange { address: u64 },

    #[error("Disk image not found: {0}")]
    DiskImageNotFound(PathBuf),

    #[error("Disk I/O error: {0}")]
    DiskIo(String),

    #[error("Kernel not found: {0}")]
    KernelNotFound(PathBuf),

    #[error("Initramfs not found: {0}")]
    InitramfsNotFound(PathBuf),

    #[error("Kernel load failed: {0}")]
    KernelLoadFailed(String),

    #[error("System image not found: {0}")]
    SystemImageNotFound(PathBuf),

    #[error("ADB connection failed: {0}")]
    AdbConnectionFailed(String),

    #[error("ADB protocol error: {0}")]
    AdbProtocolError(String),

    #[error("ADB command failed: {0}")]
    AdbCommandFailed(String),

    #[error("Display initialization failed: {0}")]
    DisplayInitFailed(String),

    #[error("Audio initialization failed: {0}")]
    AudioInitFailed(String),

    #[error("Network interface failed: {0}")]
    NetworkInterfaceFailed(String),

    #[error("NAT setup failed: {0}")]
    NatSetupFailed(String),

    #[error("Input error: {0}")]
    InputError(String),

    #[error("IPC connection failed: {0}")]
    IpcConnectionFailed(String),

    #[error("IPC protocol error: {0}")]
    IpcProtocolError(String),

    #[error("Daemon not running: {0}")]
    DaemonNotRunning(String),

    #[error("Daemon start failed: {0}")]
    DaemonStartFailed(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Device not found: {0}")]
    DeviceNotFound(String),

    #[error("Device already exists: {0}")]
    DeviceAlreadyExists(String),

    #[error("APK not found: {0}")]
    ApkNotFound(PathBuf),

    #[error("APK install failed: {0}")]
    ApkInstallFailed(String),

    #[error("APK launch failed (package={package}): {reason}")]
    ApkLaunchFailed { package: String, reason: String },

    #[error("ABI mismatch: required={required}, provided={provided}")]
    AbiMismatch { required: String, provided: String },

    #[error("Snapshot error: {0}")]
    SnapshotError(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("OS error: {0}")]
    OsError(#[from] std::io::Error),

    #[error("Not implemented: {0}")]
    NotImplemented(String),

    #[error("Unavailable: {0}")]
    Unavailable(String),
}

impl From<serde_json::Error> for LiteDroidError {
    fn from(err: serde_json::Error) -> Self {
        LiteDroidError::ConfigError(err.to_string())
    }
}

/// Convenience result type alias for the whole crate.
pub type Result<T> = std::result::Result<T, LiteDroidError>;
