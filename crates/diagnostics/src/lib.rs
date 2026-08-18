use std::ffi::{CStr, CString};
use std::fs;
use std::path::Path;
use std::process::Command;

use serde::Serialize;
use tracing::info;

use litedroid_core::DEFAULT_DATA_DIR;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticStatus {
    Ok,
    Warn,
    Error,
}

impl std::fmt::Display for DiagnosticStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiagnosticStatus::Ok => write!(f, "\u{2713}"),      // ✓
            DiagnosticStatus::Warn => write!(f, "\u{26a0}"),    // ⚠
            DiagnosticStatus::Error => write!(f, "\u{2717}"),   // ✗
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticResult {
    pub name: String,
    pub status: DiagnosticStatus,
    pub message: String,
    pub details: Option<String>,
}

impl std::fmt::Display for DiagnosticResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.details {
            Some(d) => write!(f, "{} {}: {} ({})", self.status, self.name, self.message, d),
            None => write!(f, "{} {}: {}", self.status, self.name, self.message),
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run all system diagnostics and return the collected results.
pub fn run_diagnostics() -> Vec<DiagnosticResult> {
    info!("running system diagnostics");
    let mut results = Vec::new();

    results.push(check_kernel_version());
    results.push(check_cpu_arch());
    results.push(check_cpu_virtualization());
    results.push(check_kvm_availability());
    results.push(check_kvm_api_version());
    results.push(check_available_ram());
    results.push(check_disk_space());
    results.push(check_gpu_info());
    results.push(check_network_namespace());
    results.push(check_adb_in_path());

    info!(count = results.len(), "diagnostics complete");
    results
}

// ---------------------------------------------------------------------------
// Individual checks
// ---------------------------------------------------------------------------

fn check_kernel_version() -> DiagnosticResult {
    match fs::read_to_string("/proc/version") {
        Ok(content) => {
            let version = content.trim();
            let parsed = version.split_whitespace().next().unwrap_or("unknown");
            DiagnosticResult {
                name: "Kernel Version".into(),
                status: DiagnosticStatus::Ok,
                message: parsed.to_string(),
                details: Some(version.to_string()),
            }
        }
        Err(e) => DiagnosticResult {
            name: "Kernel Version".into(),
            status: DiagnosticStatus::Error,
            message: "failed to read /proc/version".into(),
            details: Some(e.to_string()),
        },
    }
}

fn check_cpu_arch() -> DiagnosticResult {
    let mut utsname: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut utsname) } == 0 {
        let machine = unsafe {
            CStr::from_ptr(utsname.machine.as_ptr())
                .to_string_lossy()
                .to_string()
        };
        DiagnosticResult {
            name: "CPU Architecture".into(),
            status: DiagnosticStatus::Ok,
            message: machine.clone(),
            details: Some(format!("uname -m: {}", machine)),
        }
    } else {
        DiagnosticResult {
            name: "CPU Architecture".into(),
            status: DiagnosticStatus::Error,
            message: "uname failed".into(),
            details: None,
        }
    }
}

fn check_cpu_virtualization() -> DiagnosticResult {
    match fs::read_to_string("/proc/cpuinfo") {
        Ok(content) => {
            let has_vmx = content.contains("vmx");
            let has_svm = content.contains("svm");
            if has_vmx {
                DiagnosticResult {
                    name: "CPU Virtualization".into(),
                    status: DiagnosticStatus::Ok,
                    message: "Intel VT-x (vmx) supported".into(),
                    details: None,
                }
            } else if has_svm {
                DiagnosticResult {
                    name: "CPU Virtualization".into(),
                    status: DiagnosticStatus::Ok,
                    message: "AMD-V (svm) supported".into(),
                    details: None,
                }
            } else {
                DiagnosticResult {
                    name: "CPU Virtualization".into(),
                    status: DiagnosticStatus::Error,
                    message: "no CPU virtualization support detected".into(),
                    details: None,
                }
            }
        }
        Err(e) => DiagnosticResult {
            name: "CPU Virtualization".into(),
            status: DiagnosticStatus::Error,
            message: "failed to read /proc/cpuinfo".into(),
            details: Some(e.to_string()),
        },
    }
}

fn check_kvm_availability() -> DiagnosticResult {
    match fs::File::open("/dev/kvm") {
        Ok(_file) => DiagnosticResult {
            name: "KVM Availability".into(),
            status: DiagnosticStatus::Ok,
            message: "/dev/kvm is accessible".into(),
            details: None,
        },
        Err(e) => DiagnosticResult {
            name: "KVM Availability".into(),
            status: DiagnosticStatus::Error,
            message: "/dev/kvm not accessible".into(),
            details: Some(e.to_string()),
        },
    }
}

fn check_kvm_api_version() -> DiagnosticResult {
    // KVM_GET_API_VERSION = _IO(KVMIO, 0x00) = 0xAE00 on x86_64 Linux.
    match fs::File::open("/dev/kvm") {
        Ok(file) => {
            use std::os::unix::io::AsRawFd;
            let fd = file.as_raw_fd();
            let api_ver = unsafe { libc::ioctl(fd, 0xAE00) };
            if api_ver >= 0 {
                DiagnosticResult {
                    name: "KVM API Version".into(),
                    status: DiagnosticStatus::Ok,
                    message: format!("KVM API version {}", api_ver),
                    details: None,
                }
            } else {
                DiagnosticResult {
                    name: "KVM API Version".into(),
                    status: DiagnosticStatus::Error,
                    message: "KVM_GET_API_VERSION ioctl failed".into(),
                    details: Some(format!("ioctl returned {}", api_ver)),
                }
            }
        }
        Err(e) => DiagnosticResult {
            name: "KVM API Version".into(),
            status: DiagnosticStatus::Warn,
            message: "skipped (KVM not available)".into(),
            details: Some(e.to_string()),
        },
    }
}

fn check_available_ram() -> DiagnosticResult {
    match fs::read_to_string("/proc/meminfo") {
        Ok(content) => {
            let mut available_kb: u64 = 0;
            let mut found = false;
            for line in content.lines() {
                if let Some(rest) = line.strip_prefix("MemAvailable:") {
                    let parts: Vec<&str> = rest.split_whitespace().collect();
                    if let Some(&kb_str) = parts.first() {
                        available_kb = kb_str.parse().unwrap_or(0);
                        found = true;
                    }
                    break;
                }
            }
            if found {
                let available_mb = available_kb / 1024;
                let status = if available_mb >= 2048 {
                    DiagnosticStatus::Ok
                } else if available_mb >= 512 {
                    DiagnosticStatus::Warn
                } else {
                    DiagnosticStatus::Error
                };
                DiagnosticResult {
                    name: "Available RAM".into(),
                    status,
                    message: format!("{} MB available", available_mb),
                    details: Some(format!("{} kB", available_kb)),
                }
            } else {
                DiagnosticResult {
                    name: "Available RAM".into(),
                    status: DiagnosticStatus::Warn,
                    message: "MemAvailable not found in /proc/meminfo".into(),
                    details: None,
                }
            }
        }
        Err(e) => DiagnosticResult {
            name: "Available RAM".into(),
            status: DiagnosticStatus::Error,
            message: "failed to read /proc/meminfo".into(),
            details: Some(e.to_string()),
        },
    }
}

fn check_disk_space() -> DiagnosticResult {
    let data_dir_str = if DEFAULT_DATA_DIR.starts_with("~/") {
        dirs::home_dir()
            .map(|h| format!("{}/{}", h.display(), &DEFAULT_DATA_DIR[2..]))
            .unwrap_or_else(|| "/tmp".to_string())
    } else {
        DEFAULT_DATA_DIR.to_string()
    };
    let data_dir = Path::new(&data_dir_str);

    let _ = fs::create_dir_all(data_dir);

    let path_c = match CString::new(data_dir.to_string_lossy().as_ref()) {
        Ok(s) => s,
        Err(_) => {
            return DiagnosticResult {
                name: "Disk Space".into(),
                status: DiagnosticStatus::Error,
                message: "invalid data directory path".into(),
                details: None,
            };
        }
    };

    let mut stat_buf: libc::statvfs = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::statvfs(path_c.as_ptr(), &mut stat_buf) };
    if ret == 0 {
        let block_size = stat_buf.f_bsize as u64;
        let total_gb = (stat_buf.f_blocks as u64 * block_size) / (1024 * 1024 * 1024);
        let available_gb = (stat_buf.f_bavail as u64 * block_size) / (1024 * 1024 * 1024);
        let status = if available_gb >= 10 {
            DiagnosticStatus::Ok
        } else if available_gb >= 2 {
            DiagnosticStatus::Warn
        } else {
            DiagnosticStatus::Error
        };
        DiagnosticResult {
            name: "Disk Space".into(),
            status,
            message: format!("{} GB available ({} GB total)", available_gb, total_gb),
            details: Some(format!("data dir: {}", data_dir.display())),
        }
    } else {
        DiagnosticResult {
            name: "Disk Space".into(),
            status: DiagnosticStatus::Error,
            message: "statvfs failed".into(),
            details: None,
        }
    }
}

fn check_gpu_info() -> DiagnosticResult {
    match fs::read_to_string("/sys/class/drm/card0/device/vendor") {
        Ok(vendor_str) => {
            let vendor = vendor_str.trim();
            let vendor_name = match vendor {
                "0x8086" => "Intel",
                "0x10de" => "NVIDIA",
                "0x1002" => "AMD",
                _ => "Unknown",
            };
            DiagnosticResult {
                name: "GPU Info".into(),
                status: DiagnosticStatus::Ok,
                message: format!("{} ({})", vendor_name, vendor),
                details: None,
            }
        }
        Err(e) => DiagnosticResult {
            name: "GPU Info".into(),
            status: DiagnosticStatus::Warn,
            message: "GPU vendor not detected".into(),
            details: Some(e.to_string()),
        },
    }
}

fn check_network_namespace() -> DiagnosticResult {
    let fd = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_RAW,
            libc::NETLINK_KOBJECT_UEVENT,
        )
    };
    if fd >= 0 {
        unsafe {
            libc::close(fd);
        }
        DiagnosticResult {
            name: "Network Namespace".into(),
            status: DiagnosticStatus::Ok,
            message: "NETLINK socket created successfully".into(),
            details: None,
        }
    } else {
        DiagnosticResult {
            name: "Network Namespace".into(),
            status: DiagnosticStatus::Warn,
            message: "NETLINK socket creation failed (may need CAP_NET_ADMIN)".into(),
            details: None,
        }
    }
}

fn check_adb_in_path() -> DiagnosticResult {
    match Command::new("which").arg("adb").output() {
        Ok(output) => {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .to_string();
                DiagnosticResult {
                    name: "ADB in PATH".into(),
                    status: DiagnosticStatus::Ok,
                    message: format!("adb found: {}", path),
                    details: None,
                }
            } else {
                DiagnosticResult {
                    name: "ADB in PATH".into(),
                    status: DiagnosticStatus::Error,
                    message: "adb not found in PATH".into(),
                    details: None,
                }
            }
        }
        Err(e) => DiagnosticResult {
            name: "ADB in PATH".into(),
            status: DiagnosticStatus::Error,
            message: "failed to check for adb".into(),
            details: Some(e.to_string()),
        },
    }
}
