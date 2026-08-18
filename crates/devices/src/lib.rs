use std::fs::File;
use std::io::Read as _;

use parking_lot::Mutex;
use tracing::{debug, warn};

use litedroid_core::{
    LiteDroidError, Result, MAX_VIRTIO_DEVICES, VIRTIO_MMIO_SIZE, VIRTIO_MMIO_START,
};

const VIRTIO_MAGIC: u32 = 0x7472_6976; // "virt"
const VIRTIO_VERSION_2: u32 = 2;
const VIRTIO_ID_CONSOLE: u32 = 3;
const VIRTIO_ID_RNG: u32 = 4;

/// Trait for virtual devices connected via MMIO.
#[allow(unused)]
pub trait VirtDevice: Send + Sync {
    fn name(&self) -> &str;
    fn device_type(&self) -> &str;
    fn mmio_read(&mut self, offset: u64, size: u32) -> u64;
    fn mmio_write(&mut self, offset: u64, size: u32, value: u64);
    fn reset(&mut self);
    fn device_tree_compatible(&self) -> &str;
}

/// Container that routes MMIO reads/writes to devices by address range.
///
/// Devices are mapped at `VIRTIO_MMIO_START + index * VIRTIO_MMIO_SIZE`.
pub struct VirtioBus {
    devices: Mutex<Vec<Box<dyn VirtDevice>>>,
}

impl VirtioBus {
    pub fn new() -> Self {
        Self {
            devices: Mutex::new(Vec::new()),
        }
    }

    /// Add a virtual device to the bus.
    pub fn add_device(&self, device: Box<dyn VirtDevice>) -> Result<()> {
        let mut devices = self.devices.lock();
        if devices.len() >= MAX_VIRTIO_DEVICES {
            warn!("cannot add device: maximum device count reached");
            return Err(LiteDroidError::DeviceAlreadyExists(
                "maximum device count reached".to_string(),
            ));
        }
        let name = device.name().to_string();
        debug!(device = %name, "added device to virtio bus");
        devices.push(device);
        Ok(())
    }

    /// Route an MMIO read to the appropriate device.
    pub fn read(&self, address: u64, size: u32) -> u64 {
        if address < VIRTIO_MMIO_START {
            return 0;
        }
        let offset = address - VIRTIO_MMIO_START;
        let index = (offset / VIRTIO_MMIO_SIZE) as usize;
        let reg_offset = offset % VIRTIO_MMIO_SIZE;

        let mut devices = self.devices.lock();
        if let Some(device) = devices.get_mut(index) {
            device.mmio_read(reg_offset, size)
        } else {
            debug!(address = format!("{:#x}", address), "no device at address");
            0
        }
    }

    /// Route an MMIO write to the appropriate device.
    pub fn write(&self, address: u64, size: u32, value: u64) {
        if address < VIRTIO_MMIO_START {
            return;
        }
        let offset = address - VIRTIO_MMIO_START;
        let index = (offset / VIRTIO_MMIO_SIZE) as usize;
        let reg_offset = offset % VIRTIO_MMIO_SIZE;

        let mut devices = self.devices.lock();
        if let Some(device) = devices.get_mut(index) {
            device.mmio_write(reg_offset, size, value);
        } else {
            debug!(
                address = format!("{:#x}", address),
                "no device at address for write"
            );
        }
    }

    /// Returns the number of devices currently registered on the bus.
    pub fn device_count(&self) -> usize {
        self.devices.lock().len()
    }

    /// Reset all devices on the bus.
    pub fn reset_all(&self) {
        let mut devices = self.devices.lock();
        for device in devices.iter_mut() {
            device.reset();
        }
        debug!(count = devices.len(), "reset all devices on virtio bus");
    }
}

impl Default for VirtioBus {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// VirtioSerialDevice
// ---------------------------------------------------------------------------

/// Simple virtio serial console with an output callback.
pub struct VirtioSerialDevice {
    status: u32,
    interrupt_status: u32,
    output: Box<dyn Fn(&[u8]) + Send + Sync>,
}

impl VirtioSerialDevice {
    pub fn new(output: Box<dyn Fn(&[u8]) + Send + Sync>) -> Self {
        Self {
            status: 0,
            interrupt_status: 0,
            output,
        }
    }
}

impl VirtDevice for VirtioSerialDevice {
    fn name(&self) -> &str {
        "virtio-serial"
    }

    fn device_type(&self) -> &str {
        "serial"
    }

    fn mmio_read(&mut self, offset: u64, _size: u32) -> u64 {
        match offset {
            0x00 => VIRTIO_MAGIC as u64,
            0x04 => VIRTIO_VERSION_2 as u64,
            0x08 => VIRTIO_ID_CONSOLE as u64,
            0x0c => 0, // vendor
            0x34 => self.interrupt_status as u64,
            0x40 => self.status as u64,
            _ => 0,
        }
    }

    fn mmio_write(&mut self, offset: u64, size: u32, value: u64) {
        match offset {
            0x40 => {
                self.status = value as u32;
            }
            0x100 => {
                // Transmit register: send bytes through the output callback.
                let bytes = value.to_le_bytes();
                let len = (size as usize).min(8);
                (self.output)(&bytes[..len]);
            }
            0x28 => {
                // Queue notify
                debug!(queue = value, "queue notify on serial device");
            }
            _ => {
                debug!(
                    offset = format!("{:#x}", offset),
                    value = format!("{:#x}", value),
                    "unhandled serial mmio write"
                );
            }
        }
    }

    fn reset(&mut self) {
        self.status = 0;
        self.interrupt_status = 0;
    }

    fn device_tree_compatible(&self) -> &str {
        "virtio,serial"
    }
}

// ---------------------------------------------------------------------------
// VirtioRngDevice
// ---------------------------------------------------------------------------

/// Virtio RNG device that reads from `/dev/urandom` for guest entropy.
pub struct VirtioRngDevice {
    status: u32,
    urandom: File,
}

impl VirtioRngDevice {
    pub fn new() -> std::io::Result<Self> {
        let urandom = File::open("/dev/urandom")?;
        Ok(Self { status: 0, urandom })
    }
}

impl VirtDevice for VirtioRngDevice {
    fn name(&self) -> &str {
        "virtio-rng"
    }

    fn device_type(&self) -> &str {
        "rng"
    }

    fn mmio_read(&mut self, offset: u64, size: u32) -> u64 {
        match offset {
            0x00 => VIRTIO_MAGIC as u64,
            0x04 => VIRTIO_VERSION_2 as u64,
            0x08 => VIRTIO_ID_RNG as u64,
            0x0c => 0,
            0x40 => self.status as u64,
            0x100 => {
                // Data register: read random bytes from /dev/urandom.
                let mut buf = vec![0u8; size as usize];
                if self.urandom.read_exact(&mut buf).is_ok() {
                    let mut val = 0u64;
                    for &b in &buf {
                        val = (val << 8) | b as u64;
                    }
                    val
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    fn mmio_write(&mut self, offset: u64, _size: u32, value: u64) {
        match offset {
            0x40 => {
                self.status = value as u32;
            }
            _ => {
                debug!(
                    offset = format!("{:#x}", offset),
                    value = format!("{:#x}", value),
                    "unhandled rng mmio write"
                );
            }
        }
    }

    fn reset(&mut self) {
        self.status = 0;
    }

    fn device_tree_compatible(&self) -> &str {
        "virtio,rng"
    }
}
