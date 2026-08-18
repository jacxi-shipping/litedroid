use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::constants::*;
use crate::error::{LiteDroidError, Result};
use crate::types::*;

/// Pre-canned device profiles that tune resources for different use-cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceProfile {
    /// Minimal resources — good for CI or headless testing.
    Minimal,
    /// Generous resources for interactive development.
    Developer,
    /// Balanced configuration aiming for broad app compatibility.
    Compatibility,
}

/// Complete, persisted configuration for a single virtual device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    /// Human-readable name (e.g. "Pixel 7 API 34").
    pub name: String,

    /// Unique device identifier.
    pub id: Uuid,

    /// Emulated CPU architecture.
    pub architecture: Architecture,

    /// Number of virtual CPUs.
    pub vcpu_count: u32,

    /// Guest RAM size in megabytes.
    pub ram_mb: u64,

    /// Display configuration.
    pub display: DisplayConfig,

    /// Network configuration.
    pub network: NetworkConfig,

    /// Audio configuration.
    pub audio: AudioConfig,

    /// Storage configuration.
    pub storage: StorageConfig,

    /// Path to the guest Linux kernel image (bzImage / Image).
    pub kernel_path: PathBuf,

    /// Path to the initramfs / ramdisk.
    pub initramfs_path: PathBuf,

    /// Path to the Android system image.
    pub system_image_path: PathBuf,

    /// Optional path to a device-tree blob.
    pub dtb_path: Option<PathBuf>,

    /// Set of Android ABIs supported by this configuration.
    pub supported_abis: Vec<AndroidAbi>,

    /// Kernel command line (may override the default).
    pub kernel_cmdline: String,

    /// Target Android version string (e.g. "14").
    pub android_version: String,

    /// Android API level (e.g. 34).
    pub api_level: u32,

    /// Pre-canned profile this config was derived from.
    pub profile: DeviceProfile,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            name: "LiteDroid".to_string(),
            id: Uuid::new_v4(),
            architecture: Architecture::X86_64,
            vcpu_count: DEFAULT_VCPU_COUNT,
            ram_mb: DEFAULT_RAM_MB,
            display: DisplayConfig::default(),
            network: NetworkConfig::default(),
            audio: AudioConfig::default(),
            storage: StorageConfig::default(),
            kernel_path: PathBuf::new(),
            initramfs_path: PathBuf::new(),
            system_image_path: PathBuf::new(),
            dtb_path: None,
            supported_abis: vec![AndroidAbi::X86_64],
            kernel_cmdline: DEFAULT_KERNEL_CMDLINE.to_string(),
            android_version: "14".to_string(),
            api_level: 34,
            profile: DeviceProfile::Developer,
        }
    }
}

impl DeviceConfig {
    /// Validate invariants. Returns `Ok(())` or the first error found.
    pub fn validate(&self) -> Result<()> {
        if self.ram_mb < MIN_RAM_MB {
            return Err(LiteDroidError::ConfigError(format!(
                "RAM {}MB is below the minimum of {}MB",
                self.ram_mb, MIN_RAM_MB
            )));
        }
        if self.ram_mb > MAX_RAM_MB {
            return Err(LiteDroidError::ConfigError(format!(
                "RAM {}MB exceeds the maximum of {}MB",
                self.ram_mb, MAX_RAM_MB
            )));
        }
        if self.vcpu_count == 0 {
            return Err(LiteDroidError::ConfigError(
                "vCPU count must be at least 1".to_string(),
            ));
        }
        if self.supported_abis.is_empty() {
            return Err(LiteDroidError::ConfigError(
                "At least one ABI must be specified".to_string(),
            ));
        }
        Ok(())
    }
}

/// Runtime state of a single virtual device.
#[derive(Debug, Clone)]
pub struct DeviceState {
    pub id: Uuid,
    pub name: String,
    pub power_state: PowerState,
    pub boot_mode: BootMode,
    /// PID of the VMM process, if running.
    pub pid: Option<u32>,
    /// Seconds since the VM was started.
    pub uptime_secs: u64,
    /// The configuration this state was created from.
    pub config: DeviceConfig,
}

/// A lightweight summary suitable for list / status commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: Uuid,
    pub name: String,
    pub power_state: PowerState,
    pub architecture: Architecture,
    pub vcpu_count: u32,
    pub ram_mb: u64,
    pub android_version: String,
    pub api_level: u32,
}

impl From<&DeviceState> for DeviceInfo {
    fn from(state: &DeviceState) -> Self {
        Self {
            id: state.id,
            name: state.name.clone(),
            power_state: state.power_state,
            architecture: state.config.architecture,
            vcpu_count: state.config.vcpu_count,
            ram_mb: state.config.ram_mb,
            android_version: state.config.android_version.clone(),
            api_level: state.config.api_level,
        }
    }
}
