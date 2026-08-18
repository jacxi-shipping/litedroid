//! Guest physical memory backed by `mmap(2)`.
//!
//! The region is allocated with `MAP_PRIVATE | MAP_ANONYMOUS` and
//! registered with KVM via `KVM_SET_USER_MEMORY_REGION`.

use std::ptr::NonNull;

use libc::{c_void, mmap, munmap, MAP_ANONYMOUS, MAP_PRIVATE, PROT_READ, PROT_WRITE};
use tracing::debug;

use litedroid_core::error::{LiteDroidError, Result};

// ---------------------------------------------------------------------------
// GuestMemory
// ---------------------------------------------------------------------------

/// A contiguous region of guest physical memory.
pub struct GuestMemory {
    /// Non-null pointer returned by `mmap`.
    ptr: NonNull<c_void>,
    /// Region length in bytes.
    size: u64,
}

// SAFETY: `GuestMemory` owns a unique `mmap` region that is independent of
// any thread.  It can be sent across threads safely.
unsafe impl Send for GuestMemory {}
// SAFETY: All mutation goes through `&self` methods that take `&[u8]` / `&mut
// [u8]` with explicit bounds checking, and no interior mutability is
// exposed.
unsafe impl Sync for GuestMemory {}

impl GuestMemory {
    /// Allocate `size_bytes` bytes of guest RAM via anonymous mmap.
    pub fn new(size_bytes: u64) -> Result<Self> {
        if size_bytes == 0 {
            return Err(LiteDroidError::MemoryAllocationFailed {
                requested_mb: 0,
                available_mb: 0,
            });
        }

        let ptr = unsafe {
            mmap(
                std::ptr::null_mut::<c_void>(),
                size_bytes as usize,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                -1, // fd — unused with MAP_ANONYMOUS
                0,  // offset
            )
        };

        if ptr == libc::MAP_FAILED {
            return Err(LiteDroidError::MemoryMapFailed(format!(
                "mmap failed for {} bytes",
                size_bytes
            )));
        }

        let ptr = NonNull::new(ptr).expect("mmap returned non-null on success");

        debug!("GuestMemory allocated: {} bytes at {:p}", size_bytes, ptr.as_ptr());

        Ok(Self { ptr, size: size_bytes })
    }

    /// Copy `buf.len()` bytes **from** guest physical address `addr` into the
    /// caller-supplied buffer.
    ///
    /// # Panics / Errors
    /// Returns [`LiteDroidError::GuestAddressOutOfRange`] when the range
    /// `[addr, addr + buf.len())` is not fully contained in the allocation.
    pub fn read_guest(&self, addr: u64, buf: &mut [u8]) -> Result<()> {
        let end = addr.checked_add(buf.len() as u64).ok_or_else(|| {
            LiteDroidError::GuestAddressOutOfRange { address: addr }
        })?;
        if end > self.size {
            return Err(LiteDroidError::GuestAddressOutOfRange { address: addr });
        }

        // SAFETY: We just verified that the range is within the allocation,
        // and the region was created with PROT_READ.
        unsafe {
            let src = (self.ptr.as_ptr() as *const u8).add(addr as usize);
            std::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), buf.len());
        }
        Ok(())
    }

    /// Copy the contents of `data` **into** guest physical memory starting at
    /// `addr`.
    ///
    /// # Errors
    /// Returns [`LiteDroidError::GuestAddressOutOfRange`] when the range
    /// `[addr, addr + data.len())` is not fully contained in the allocation.
    pub fn write_guest(&self, addr: u64, data: &[u8]) -> Result<()> {
        let end = addr.checked_add(data.len() as u64).ok_or_else(|| {
            LiteDroidError::GuestAddressOutOfRange { address: addr }
        })?;
        if end > self.size {
            return Err(LiteDroidError::GuestAddressOutOfRange { address: addr });
        }

        // SAFETY: bounds verified above; region created with PROT_WRITE.
        unsafe {
            let dst = (self.ptr.as_ptr() as *mut u8).add(addr as usize);
            std::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
        }
        Ok(())
    }

    /// Convenience helper: load a binary blob (kernel, initramfs, …) at the
    /// given guest physical address.
    pub fn load_blob(&self, addr: u64, data: &[u8]) -> Result<()> {
        self.write_guest(addr, data)
    }

    /// Raw pointer to the start of the guest region (for KVM user-memory
    /// registration, etc.).
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr.as_ptr() as *const u8
    }

    /// The host-virtual address cast to `u64` — this is what KVM expects in
    /// `kvm_userspace_memory_region.userspace_addr`.
    pub fn userspace_addr(&self) -> u64 {
        self.ptr.as_ptr() as u64
    }

    /// Size of the allocation in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }
}

impl Drop for GuestMemory {
    fn drop(&mut self) {
        debug!(
            "GuestMemory munmap: {} bytes at {:p}",
            self.size,
            self.ptr.as_ptr()
        );
        // SAFETY: `self.ptr` and `self.size` were obtained from a successful
        // `mmap` call and have not been modified since.
        let ret = unsafe {
            munmap(self.ptr.as_ptr(), self.size as usize)
        };
        if ret != 0 {
            tracing::error!("munmap failed for guest memory region");
        }
    }
}
