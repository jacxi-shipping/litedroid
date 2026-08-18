use std::ffi::CString;
use std::io;
use std::process::Command;

use libc::{
    c_int, c_void, close, ioctl, open, read, sockaddr_in, write, AF_INET, IFNAMSIZ, O_RDWR,
};
use litedroid_core::{LiteDroidError, Result};
use litedroid_devices::VirtDevice;
use parking_lot::Mutex;
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const TUN_IFF_TAP: i32 = 0x0002;
const TUN_IFF_NO_PI: i32 = 0x0800;
const NET_IFF_UP: i32 = 0x0001;
const TUNSETIFF: libc::c_ulong = 0x4004_54CA;
const SIOCSIFFLAGS: libc::c_ulong = 0x8914;
const SIOCSIFADDR: libc::c_ulong = 0x8916;
const SIOCSIFNETMASK: libc::c_ulong = 0x891C;

const VIRTIO_NET_MAGIC: u32 = 0x7472_6976;
const VIRTIO_NET_VERSION: u32 = 2;
const VIRTIO_ID_NET: u32 = 1;

// ---------------------------------------------------------------------------
// ifreq helper
// ---------------------------------------------------------------------------

/// Minimal `struct ifreq` matching the Linux kernel layout (40 bytes).
#[repr(C)]
struct IfReq {
    name: [u8; IFNAMSIZ as usize],
    data: [u8; 24],
}

impl IfReq {
    fn new() -> Self {
        Self {
            name: [0u8; IFNAMSIZ as usize],
            data: [0u8; 24],
        }
    }

    fn set_name(&mut self, name: &str) {
        let bytes = name.as_bytes();
        let len = bytes.len().min(self.name.len() - 1);
        self.name[..len].copy_from_slice(&bytes[..len]);
        // NUL-terminate
        self.name[len] = 0;
    }

    fn get_name(&self) -> String {
        let end = self
            .name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.name.len());
        String::from_utf8_lossy(&self.name[..end]).into_owned()
    }

    fn set_flags(&mut self, flags: i32) {
        let bytes = flags.to_ne_bytes();
        self.data[0] = bytes[0];
        self.data[1] = bytes[1];
    }

    fn set_sockaddr_in(&mut self, sa: &libc::sockaddr_in) {
        let src = sa as *const libc::sockaddr_in as *const u8;
        unsafe {
            std::ptr::copy_nonoverlapping(
                src,
                self.data.as_mut_ptr(),
                std::mem::size_of::<libc::sockaddr_in>(),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// TapInterface
// ---------------------------------------------------------------------------

/// TAP network interface for guest networking.
pub struct TapInterface {
    fd: c_int,
    name: String,
}

impl TapInterface {
    /// Create a new TAP interface with the given name (e.g. "litedroid-tap0").
    pub fn new(iface_name: &str) -> Result<Self> {
        let tun_cstr = CString::new("/dev/net/tun").unwrap();
        let fd = unsafe { open(tun_cstr.as_ptr(), O_RDWR) };
        if fd < 0 {
            return Err(LiteDroidError::NetworkInterfaceFailed(format!(
                "failed to open /dev/net/tun: {}",
                io::Error::last_os_error()
            )));
        }

        let mut ifr = IfReq::new();
        ifr.set_name(iface_name);
        ifr.set_flags(TUN_IFF_TAP | TUN_IFF_NO_PI);

        let ret = unsafe { ioctl(fd, TUNSETIFF, &mut ifr) };
        if ret < 0 {
            unsafe { close(fd) };
            return Err(LiteDroidError::NetworkInterfaceFailed(format!(
                "TUNSETIFF failed: {}",
                io::Error::last_os_error()
            )));
        }

        let actual_name = ifr.get_name();
        debug!(name = %actual_name, "created TAP interface");
        Ok(Self {
            fd,
            name: actual_name,
        })
    }

    /// The name assigned to this interface by the kernel.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Read a single packet from the TAP interface.
    pub fn read_packet(&self, buf: &mut [u8]) -> Result<usize> {
        let n = unsafe { read(self.fd, buf.as_mut_ptr() as *mut c_void, buf.len()) };
        if n < 0 {
            return Err(LiteDroidError::NetworkInterfaceFailed(format!(
                "TAP read failed: {}",
                io::Error::last_os_error()
            )));
        }
        Ok(n as usize)
    }

    /// Write a single packet to the TAP interface.
    pub fn write_packet(&self, data: &[u8]) -> Result<usize> {
        let n = unsafe { write(self.fd, data.as_ptr() as *const c_void, data.len()) };
        if n < 0 {
            return Err(LiteDroidError::NetworkInterfaceFailed(format!(
                "TAP write failed: {}",
                io::Error::last_os_error()
            )));
        }
        Ok(n as usize)
    }

    /// Bring the interface up (IFF_UP).
    pub fn set_up(&self) -> Result<()> {
        let mut ifr = IfReq::new();
        ifr.set_name(&self.name);
        ifr.set_flags(NET_IFF_UP);

        let ret = unsafe { ioctl(self.fd, SIOCSIFFLAGS, &ifr) };
        if ret < 0 {
            return Err(LiteDroidError::NetworkInterfaceFailed(format!(
                "SIOCSIFFLAGS failed for {}: {}",
                self.name,
                io::Error::last_os_error()
            )));
        }
        info!(name = %self.name, "brought TAP interface up");
        Ok(())
    }

    /// Assign an IPv4 address to the interface (e.g. "10.0.2.1").
    pub fn set_ip_address(&self, addr: &str) -> Result<()> {
        let in_addr = parse_ip(addr)?;
        let mut sa: sockaddr_in = unsafe { std::mem::zeroed() };
        sa.sin_family = AF_INET as u16;
        sa.sin_addr = in_addr;

        let mut ifr = IfReq::new();
        ifr.set_name(&self.name);
        ifr.set_sockaddr_in(&sa);

        let ret = unsafe { ioctl(self.fd, SIOCSIFADDR, &ifr) };
        if ret < 0 {
            return Err(LiteDroidError::NetworkInterfaceFailed(format!(
                "SIOCSIFADDR failed for {}: {}",
                self.name,
                io::Error::last_os_error()
            )));
        }
        info!(name = %self.name, addr, "set TAP IP address");
        Ok(())
    }

    /// Set the network mask (e.g. "255.255.255.0").
    pub fn set_netmask(&self, mask: &str) -> Result<()> {
        let in_addr = parse_ip(mask)?;
        let mut sa: sockaddr_in = unsafe { std::mem::zeroed() };
        sa.sin_family = AF_INET as u16;
        sa.sin_addr = in_addr;

        let mut ifr = IfReq::new();
        ifr.set_name(&self.name);
        ifr.set_sockaddr_in(&sa);

        let ret = unsafe { ioctl(self.fd, SIOCSIFNETMASK, &ifr) };
        if ret < 0 {
            return Err(LiteDroidError::NetworkInterfaceFailed(format!(
                "SIOCSIFNETMASK failed for {}: {}",
                self.name,
                io::Error::last_os_error()
            )));
        }
        info!(name = %self.name, mask, "set TAP netmask");
        Ok(())
    }

    /// Raw file descriptor for the TAP device.
    pub fn fd(&self) -> i32 {
        self.fd
    }
}

impl Drop for TapInterface {
    fn drop(&mut self) {
        if self.fd >= 0 {
            if unsafe { close(self.fd) } < 0 {
                error!(name = %self.name, "failed to close TAP fd");
            }
        }
    }
}

/// Parse an IPv4 address string into a `libc::in_addr`.
fn parse_ip(addr: &str) -> Result<libc::in_addr> {
    let ip: std::net::Ipv4Addr = addr
        .parse()
        .map_err(|_| LiteDroidError::NetworkInterfaceFailed(format!("invalid IP: {addr}")))?;
    let s_addr = u32::from_be_bytes(ip.octets());
    Ok(libc::in_addr { s_addr })
}

// ---------------------------------------------------------------------------
// VirtioNetDevice
// ---------------------------------------------------------------------------

/// Virtio network device backed by a [`TapInterface`].
pub struct VirtioNetDevice {
    #[allow(dead_code)]
    tap: TapInterface,
    mac_address: [u8; 6],
    status: u32,
}

impl VirtioNetDevice {
    pub fn new(tap: TapInterface, mac_address: [u8; 6]) -> Self {
        Self {
            tap,
            mac_address,
            status: 0,
        }
    }

    fn mac_as_u64(&self) -> u64 {
        let mut v = 0u64;
        for &b in &self.mac_address {
            v = (v << 8) | b as u64;
        }
        v
    }
}

impl VirtDevice for VirtioNetDevice {
    fn name(&self) -> &str {
        "virtio-net"
    }

    fn device_type(&self) -> &str {
        "net"
    }

    fn mmio_read(&mut self, offset: u64, _size: u32) -> u64 {
        match offset {
            0x00 => VIRTIO_NET_MAGIC as u64,
            0x04 => VIRTIO_NET_VERSION as u64,
            0x08 => VIRTIO_ID_NET as u64,
            0x0C => 0, // vendor
            0x24 => 1, // queue_ready
            0x30 => self.status as u64,
            0x100 => self.mac_as_u64(), // config: MAC address
            _ => 0,
        }
    }

    #[allow(unused)]
    fn mmio_write(&mut self, offset: u64, _size: u32, value: u64) {
        match offset {
            0x28 => {
                debug!(queue = value, "queue notify on virtio-net");
            }
            0x30 => {
                self.status = value as u32;
            }
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.status = 0;
    }

    fn device_tree_compatible(&self) -> &str {
        "virtio,net"
    }
}

// ---------------------------------------------------------------------------
// NatManager
// ---------------------------------------------------------------------------

/// Manages NAT / masquerade rules so the guest can reach the external network.
pub struct NatManager {
    tap_name: Mutex<Option<String>>,
    is_setup: std::sync::atomic::AtomicBool,
}

impl NatManager {
    /// Create a new (not yet configured) NAT manager.
    pub fn new() -> Self {
        Self {
            tap_name: Mutex::new(None),
            is_setup: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Enable IP forwarding, add MASQUERADE and FORWARD iptables rules.
    pub fn setup(&self, tap_name: &str) -> Result<Self> {
        let host_iface = self.host_interface().unwrap_or_else(|_| "eth0".to_string());

        // Enable IP forwarding
        if let Err(e) = std::fs::write("/proc/sys/net/ipv4/ip_forward", "1") {
            warn!("failed to enable IP forwarding: {e}");
        }

        // iptables MASQUERADE
        let status = Command::new("iptables")
            .args([
                "-t",
                "nat",
                "-A",
                "POSTROUTING",
                "-o",
                &host_iface,
                "-s",
                "10.0.2.0/24",
                "-j",
                "MASQUERADE",
            ])
            .status()
            .map_err(|e| LiteDroidError::NatSetupFailed(format!("iptables masquerade: {e}")));
        if let Err(e) = status {
            warn!("iptables MASQUERADE failed: {e}");
        }

        // FORWARD in
        let status = Command::new("iptables")
            .args(["-I", "FORWARD", "-i", tap_name, "-j", "ACCEPT"])
            .status()
            .map_err(|e| LiteDroidError::NatSetupFailed(format!("iptables forward-in: {e}")));
        if let Err(e) = status {
            warn!("iptables FORWARD -i failed: {e}");
        }

        // FORWARD out
        let status = Command::new("iptables")
            .args(["-I", "FORWARD", "-o", tap_name, "-j", "ACCEPT"])
            .status()
            .map_err(|e| LiteDroidError::NatSetupFailed(format!("iptables forward-out: {e}")));
        if let Err(e) = status {
            warn!("iptables FORWARD -o failed: {e}");
        }

        *self.tap_name.lock() = Some(tap_name.to_string());
        self.is_setup
            .store(true, std::sync::atomic::Ordering::Relaxed);
        info!(tap = tap_name, host = %host_iface, "NAT rules installed");
        Ok(Self {
            tap_name: Mutex::new(Some(tap_name.to_string())),
            is_setup: std::sync::atomic::AtomicBool::new(true),
        })
    }

    /// Remove the iptables rules that were added by [`setup`](Self::setup).
    pub fn teardown(&self) -> Result<()> {
        if !self
            .is_setup
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            return Ok(());
        }
        let tap = {
            let mut guard = self.tap_name.lock();
            guard.take()
        };
        let Some(tap_name) = tap else {
            return Ok(());
        };
        let host_iface = self.host_interface().unwrap_or_else(|_| "eth0".to_string());

        let _ = Command::new("iptables")
            .args([
                "-t",
                "nat",
                "-D",
                "POSTROUTING",
                "-o",
                &host_iface,
                "-s",
                "10.0.2.0/24",
                "-j",
                "MASQUERADE",
            ])
            .status();
        let _ = Command::new("iptables")
            .args(["-D", "FORWARD", "-i", &tap_name, "-j", "ACCEPT"])
            .status();
        let _ = Command::new("iptables")
            .args(["-D", "FORWARD", "-o", &tap_name, "-j", "ACCEPT"])
            .status();

        info!(tap = %tap_name, "NAT rules removed");
        Ok(())
    }

    /// Determine the host interface used for the default route by parsing
    /// `/proc/net/route`.
    pub fn host_interface(&self) -> Result<String> {
        let content = std::fs::read_to_string("/proc/net/route")
            .map_err(|e| LiteDroidError::NatSetupFailed(format!("read /proc/net/route: {e}")));
        let content = content?;

        for line in content.lines().skip(1) {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() >= 2 && fields[1] == "00000000" {
                return Ok(fields[0].to_string());
            }
        }
        Err(LiteDroidError::NatSetupFailed(
            "no default route found in /proc/net/route".into(),
        ))
    }
}

impl Default for NatManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for NatManager {
    fn drop(&mut self) {
        if self.is_setup.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = self.teardown();
        }
    }
}
