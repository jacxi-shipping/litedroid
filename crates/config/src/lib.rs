use std::fs;
use std::io::Write as _;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use litedroid_core::{
    DAEMON_SOCKET_PATH, DEFAULT_CONFIG_DIR, DEFAULT_DATA_DIR, DeviceConfig, LiteDroidError, Result,
};

/// Expand a leading `~` to the user's home directory.
pub fn expand_tilde(path: &str) -> String {
    if path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return format!("{}/{}", home.display(), &path[2..]);
        }
    }
    path.to_string()
}

/// Global daemon-level configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalConfig {
    pub socket_path: String,
    pub log_level: String,
    pub data_dir: String,
    pub config_dir: String,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            socket_path: DAEMON_SOCKET_PATH.to_string(),
            log_level: "info".to_string(),
            data_dir: DEFAULT_DATA_DIR.to_string(),
            config_dir: DEFAULT_CONFIG_DIR.to_string(),
        }
    }
}

/// Top-level configuration loaded from `~/.config/litedroid/config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiteDroidConfig {
    pub global: GlobalConfig,
}

impl Default for LiteDroidConfig {
    fn default() -> Self {
        Self {
            global: GlobalConfig::default(),
        }
    }
}

impl LiteDroidConfig {
    /// Load configuration from `~/.config/litedroid/config.toml`.
    /// Falls back to defaults when the file does not exist.
    pub fn load() -> Result<Self> {
        let config_dir = expand_tilde(DEFAULT_CONFIG_DIR);
        let full_path = format!("{}/config.toml", config_dir);
        let path = PathBuf::from(&full_path);

        if !path.exists() {
            debug!("no config file found at {}, using defaults", full_path);
            let config = Self::default();
            config.ensure_layout()?;
            return Ok(config);
        }

        let content = fs::read_to_string(&path)?;
        let config: LiteDroidConfig = toml::from_str(&content)
            .map_err(|e| LiteDroidError::ConfigError(e.to_string()))?;

        config.ensure_layout()?;
        info!(path = %full_path, "loaded configuration");
        Ok(config)
    }

    /// Create the directories used by LiteDroid on first run.
    pub fn ensure_layout(&self) -> Result<()> {
        let config_dir = expand_tilde(&self.global.config_dir);
        fs::create_dir_all(config_dir)?;
        fs::create_dir_all(self.data_dir())?;
        fs::create_dir_all(self.devices_dir())?;
        fs::create_dir_all(self.images_dir())?;
        fs::create_dir_all(self.snapshots_dir())?;
        fs::create_dir_all(self.data_dir().join("logs"))?;
        fs::create_dir_all(self.data_dir().join("run"))?;
        Ok(())
    }

    /// Persist the current configuration to disk.
    pub fn save(&self) -> Result<()> {
        self.ensure_layout()?;
        let config_dir = expand_tilde(&self.global.config_dir);
        let dir = PathBuf::from(&config_dir);
        fs::create_dir_all(&dir)?;

        let full_path = dir.join("config.toml");
        let content = toml::to_string_pretty(self)
            .map_err(|e| LiteDroidError::ConfigError(e.to_string()))?;

        let mut file = fs::File::create(&full_path)?;
        file.write_all(content.as_bytes())?;

        info!(path = %full_path.display(), "saved configuration");
        Ok(())
    }

    /// Returns the expanded data directory path.
    pub fn data_dir(&self) -> PathBuf {
        PathBuf::from(expand_tilde(&self.global.data_dir))
    }

    /// Returns the directory that stores per-device configurations.
    pub fn devices_dir(&self) -> PathBuf {
        self.data_dir().join("devices")
    }

    /// Returns the directory that stores disk images.
    pub fn images_dir(&self) -> PathBuf {
        self.data_dir().join("images")
    }

    /// Returns the directory that stores VM snapshots.
    pub fn snapshots_dir(&self) -> PathBuf {
        self.data_dir().join("snapshots")
    }

    /// Build a [`DeviceConfig`] with paths resolved against this configuration.
    pub fn default_device_config(&self) -> DeviceConfig {
        let mut config = DeviceConfig::default();
        config.storage.userdata_path = self
            .images_dir()
            .join("userdata.img")
            .to_string_lossy()
            .to_string();
        config.storage.cache_path = self
            .images_dir()
            .join("cache.img")
            .to_string_lossy()
            .to_string();
        config.kernel_path = self.images_dir().join("kernel");
        config.initramfs_path = self.images_dir().join("ramdisk.img");
        config.system_image_path = self.images_dir().join("system.img");
        config
    }
}
