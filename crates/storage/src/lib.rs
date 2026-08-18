use std::path::Path;

use libc::{
    c_int, c_void, close, fstat, ftruncate, fsync, open, pread, pwrite, size_t, stat, off_t,
    O_CLOEXEC, O_CREAT, O_RDWR,
};
use litedroid_core::{LiteDroidError, Result};
use litedroid_devices::VirtDevice;
use tracing::{debug, error};

const SECTOR_SIZE: u64 = 512;
const VIRTIO_BLK_MAGIC: u32 = 0x7472_6976;
const VIRTIO_BLK_VERSION: u32 = 2;
const VIRTIO_ID_BLOCK: u32 = 2;

// ---------------------------------------------------------------------------
// RawDiskImage
// ---------------------------------------------------------------------------

/// Raw disk image backed by a host file descriptor with positional I/O.
pub struct RawDiskImage {
    fd: c_int,
    size: u64,
}

impl RawDiskImage {
    /// Create a new disk image at `path` with the given size in bytes.
    pub fn create(path: &Path, size_bytes: u64) -> Result<Self> {
        let path_cstr =
            std::ffi::CString::new(path.to_string_lossy().as_ref()).map_err(|e| {
                LiteDroidError::DiskIo(format!("invalid path for disk image: {e}"))
            })?;

        let fd = unsafe {
            open(
                path_cstr.as_ptr(),
                O_CREAT | O_RDWR | O_CLOEXEC,
                0o644,
            )
        };

        if fd < 0 {
            return Err(LiteDroidError::DiskIo(format!(
                "failed to create disk image at {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            )));
        }

        if unsafe { ftruncate(fd, size_bytes as off_t) } < 0 {
            unsafe { close(fd) };
            return Err(LiteDroidError::DiskIo(format!(
                "ftruncate failed for {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            )));
        }

        debug!(path = %path.display(), size_bytes, "created disk image");
        Ok(Self { fd, size: size_bytes })
    }

    /// Open an existing disk image for reading and writing.
    pub fn open(path: &Path) -> Result<Self> {
        let path_cstr =
            std::ffi::CString::new(path.to_string_lossy().as_ref()).map_err(|e| {
                LiteDroidError::DiskIo(format!("invalid path for disk image: {e}"))
            })?;

        let fd = unsafe { open(path_cstr.as_ptr(), O_RDWR | O_CLOEXEC) };

        if fd < 0 {
            return Err(LiteDroidError::DiskIo(format!(
                "failed to open disk image at {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            )));
        }

        let mut st: stat = unsafe { std::mem::zeroed() };
        if unsafe { fstat(fd, &mut st) } < 0 {
            unsafe { close(fd) };
            return Err(LiteDroidError::DiskIo(format!(
                "fstat failed for {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            )));
        }

        let size = st.st_size as u64;
        debug!(path = %path.display(), size, "opened disk image");
        Ok(Self { fd, size })
    }

    /// Read bytes from the disk image at the given byte offset.
    pub fn read(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let n = unsafe {
            pread(
                self.fd,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as size_t,
                offset as off_t,
            )
        };
        if n < 0 {
            return Err(LiteDroidError::DiskIo(format!(
                "pread failed at offset {offset}: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(n as usize)
    }

    /// Write bytes to the disk image at the given byte offset.
    pub fn write(&self, offset: u64, data: &[u8]) -> Result<usize> {
        let n = unsafe {
            pwrite(
                self.fd,
                data.as_ptr() as *const c_void,
                data.len() as size_t,
                offset as off_t,
            )
        };
        if n < 0 {
            return Err(LiteDroidError::DiskIo(format!(
                "pwrite failed at offset {offset}: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(n as usize)
    }

    /// Flush pending writes to durable storage (fsync).
    pub fn flush(&self) -> Result<()> {
        if unsafe { fsync(self.fd) } < 0 {
            return Err(LiteDroidError::DiskIo(format!(
                "fsync failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(())
    }

    /// Total size of the disk image in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Number of 512-byte sectors.
    pub fn sectors(&self) -> u64 {
        self.size / SECTOR_SIZE
    }
}

impl Drop for RawDiskImage {
    fn drop(&mut self) {
        if self.fd >= 0 {
            if unsafe { close(self.fd) } < 0 {
                error!(fd = self.fd, "failed to close disk image fd");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// VirtioBlkDevice
// ---------------------------------------------------------------------------

/// Virtio block device wrapping a [`RawDiskImage`].
pub struct VirtioBlkDevice {
    disk: RawDiskImage,
    status: u32,
}

impl VirtioBlkDevice {
    pub fn new(disk: RawDiskImage) -> Self {
        Self { disk, status: 0 }
    }
}

impl VirtDevice for VirtioBlkDevice {
    fn name(&self) -> &str {
        "virtio-blk"
    }

    fn device_type(&self) -> &str {
        "block"
    }

    fn mmio_read(&mut self, offset: u64, _size: u32) -> u64 {
        match offset {
            0x00 => VIRTIO_BLK_MAGIC as u64,
            0x04 => VIRTIO_BLK_VERSION as u64,
            0x08 => VIRTIO_ID_BLOCK as u64,
            0x0C => 0, // vendor
            0x24 => 1, // queue_ready
            0x30 => self.status as u64,
            0x100 => self.disk.sectors(), // config: capacity in sectors
            _ => 0,
        }
    }

    #[allow(unused)]
    fn mmio_write(&mut self, offset: u64, _size: u32, value: u64) {
        match offset {
            0x28 => {
                // queue_notify — simplified: log and no-op
                debug!(queue = value, "queue notify on virtio-blk");
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
        "virtio,block"
    }
}
