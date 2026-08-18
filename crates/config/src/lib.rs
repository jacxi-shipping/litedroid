use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use litedroid_core::{
    DeviceConfig, LiteDroidError, Result, DAEMON_SOCKET_PATH, DEFAULT_CONFIG_DIR, DEFAULT_DATA_DIR,
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
        let config: LiteDroidConfig =
            toml::from_str(&content).map_err(|e| LiteDroidError::ConfigError(e.to_string()))?;

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
        let content =
            toml::to_string_pretty(self).map_err(|e| LiteDroidError::ConfigError(e.to_string()))?;

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

    /// Load a persisted device profile and resolve its shared image paths.
    pub fn device_config(&self, name: &str) -> Result<DeviceConfig> {
        let metadata_path = self.devices_dir().join(name).join("metadata.json");
        let metadata = fs::read_to_string(&metadata_path)
            .map_err(|_| LiteDroidError::DeviceNotFound(name.to_string()))?;
        let metadata: serde_json::Value = serde_json::from_str(&metadata)
            .map_err(|error| LiteDroidError::ConfigError(error.to_string()))?;
        let mut config = self.default_device_config();
        config.name = metadata
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(name)
            .to_string();
        if let Some(value) = metadata
            .get("vcpu_count")
            .and_then(serde_json::Value::as_u64)
        {
            config.vcpu_count = value as u32;
        }
        if let Some(value) = metadata.get("ram_mb").and_then(serde_json::Value::as_u64) {
            config.ram_mb = value;
        }
        if let Some(value) = metadata
            .get("android_version")
            .and_then(serde_json::Value::as_str)
        {
            config.android_version = value.to_string();
        }
        if let Some(value) = metadata
            .get("api_level")
            .and_then(serde_json::Value::as_u64)
        {
            config.api_level = value as u32;
        }
        config.validate()?;
        Ok(config)
    }

    /// Create the default device profile on first use and return its configuration.
    pub fn ensure_default_device(&self) -> Result<DeviceConfig> {
        let directory = self.devices_dir().join("default");
        let metadata_path = directory.join("metadata.json");
        if metadata_path.is_file() {
            return self.device_config("default");
        }
        let mut config = self.default_device_config();
        config.name = "default".to_string();
        fs::create_dir_all(&directory)?;
        let metadata = serde_json::json!({
            "name": config.name,
            "id": config.id.to_string(),
            "android_version": config.android_version,
            "api_level": config.api_level,
            "ram_mb": config.ram_mb,
            "vcpu_count": config.vcpu_count,
        });
        fs::write(
            metadata_path,
            serde_json::to_string_pretty(&metadata)
                .map_err(|error| LiteDroidError::ConfigError(error.to_string()))?,
        )?;
        Ok(config)
    }

    /// Download and install the official Android SDK system image required by LiteDroid.
    pub fn ensure_android_images(&self, api_level: u32) -> Result<()> {
        let images_dir = self.images_dir();
        fs::create_dir_all(&images_dir)?;
        if images_are_ready(&images_dir) {
            return Ok(());
        }

        let sdk_root = sdk_root(self).ok_or_else(|| {
            LiteDroidError::ConfigError(
                "Android SDK not found. Set ANDROID_HOME/ANDROID_SDK_ROOT or install command-line tools."
                    .to_string(),
            )
        })?;
        let sdkmanager = find_sdkmanager(&sdk_root).ok_or_else(|| {
            LiteDroidError::ConfigError(format!(
                "sdkmanager not found under {}. Install Android SDK command-line tools.",
                sdk_root.display()
            ))
        })?;
        let package = format!("system-images;android-{api_level};default;x86_64");

        info!(%package, "installing Android SDK system image");
        let mut licenses = Command::new(&sdkmanager)
            .arg("--sdk_root")
            .arg(&sdk_root)
            .arg("--licenses")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| LiteDroidError::ConfigError(format!("starting sdkmanager: {e}")))?;
        if let Some(mut stdin) = licenses.stdin.take() {
            stdin.write_all(&vec![b'y', b'\n'].repeat(64))?;
        }
        if !licenses.wait()?.success() {
            return Err(LiteDroidError::ConfigError(
                "Android SDK licenses were not accepted".to_string(),
            ));
        }

        let install = Command::new(&sdkmanager)
            .arg("--sdk_root")
            .arg(&sdk_root)
            .arg(&package)
            .output()?;
        if !install.status.success() {
            return Err(LiteDroidError::ConfigError(format!(
                "sdkmanager could not install {package}: {}",
                String::from_utf8_lossy(&install.stderr).trim()
            )));
        }

        let package_dir = sdk_root
            .join("system-images")
            .join(format!("android-{api_level}"))
            .join("default")
            .join("x86_64");
        copy_image(
            &package_dir,
            &["kernel-ranchu", "kernel"],
            &images_dir.join("kernel"),
        )?;
        copy_image(
            &package_dir,
            &["ramdisk.img"],
            &images_dir.join("ramdisk.img"),
        )?;
        copy_image(
            &package_dir,
            &["system.img"],
            &images_dir.join("system.img"),
        )?;
        Ok(())
    }
}

fn is_non_empty(path: &Path) -> bool {
    path.is_file() && fs::metadata(path).map(|m| m.len() > 0).unwrap_or(false)
}

fn images_are_ready(images_dir: &Path) -> bool {
    let kernel = images_dir.join("kernel");
    let ramdisk = images_dir.join("ramdisk.img");
    let system = images_dir.join("system.img");
    is_non_empty(&kernel)
        && fs::read(&kernel)
            .map(|data| data.iter().any(|byte| *byte != 0))
            .unwrap_or(false)
        && is_non_empty(&ramdisk)
        && is_non_empty(&system)
        && fs::File::open(&system)
            .and_then(|mut file| {
                use std::io::{Read, Seek, SeekFrom};
                let mut header = [0u8; 4];
                file.read_exact(&mut header)?;
                if header == [0x3a, 0xff, 0x26, 0xed] {
                    return Ok(true);
                }
                file.seek(SeekFrom::Start(0x200))?;
                let mut gpt_magic = [0u8; 8];
                file.read_exact(&mut gpt_magic)?;
                if &gpt_magic == b"EFI PART" {
                    return Ok(true);
                }
                file.seek(SeekFrom::Start(0x438))?;
                let mut magic = [0u8; 2];
                file.read_exact(&mut magic)?;
                Ok(magic == [0x53, 0xef])
            })
            .unwrap_or(false)
}

fn sdk_root(config: &LiteDroidConfig) -> Option<PathBuf> {
    std::env::var_os("ANDROID_SDK_ROOT")
        .or_else(|| std::env::var_os("ANDROID_HOME"))
        .map(PathBuf::from)
        .or_else(|| Some(config.data_dir().join("android-sdk")).filter(|p| p.exists()))
}

fn find_sdkmanager(sdk_root: &Path) -> Option<PathBuf> {
    let candidates = [
        sdk_root.join("cmdline-tools/latest/bin/sdkmanager"),
        sdk_root.join("cmdline-tools/bin/sdkmanager"),
        sdk_root.join("tools/bin/sdkmanager"),
    ];
    candidates.into_iter().find(|p| p.is_file()).or_else(|| {
        Command::new("sdkmanager")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()
            .filter(|status| status.success())
            .map(|_| PathBuf::from("sdkmanager"))
    })
}

fn copy_image(package_dir: &Path, names: &[&str], destination: &Path) -> Result<()> {
    let source = names
        .iter()
        .map(|name| package_dir.join(name))
        .find(|path| is_non_empty(path))
        .ok_or_else(|| {
            LiteDroidError::ConfigError(format!(
                "Android SDK package is missing {} in {}",
                names.join(" or "),
                package_dir.display()
            ))
        })?;
    fs::copy(source, destination)?;
    Ok(())
}
