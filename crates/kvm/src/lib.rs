pub mod cpuid;

use std::collections::HashMap;

use kvm_bindings::CpuId;
use kvm_ioctls::{Cap, VcpuExit};
use litedroid_core::error::{LiteDroidError, Result};
use tracing::{debug, info, trace, warn};

// ---------------------------------------------------------------------------
// KvmCapabilities
// ---------------------------------------------------------------------------

/// Probes KVM capabilities at construction time and remembers which are
/// available on the host.
#[derive(Debug)]
pub struct KvmCapabilities {
    flags: HashMap<&'static str, bool>,
}

impl KvmCapabilities {
    /// Probe the underlying KVM device and record every known capability.
    fn new(kvm: &kvm_ioctls::Kvm) -> Self {
        let mut flags = HashMap::new();

        let probes: &[(&str, Cap)] = &[
            ("IRQCHIP", Cap::Irqchip),
            ("HLT", Cap::Hlt),
            ("MMU_SHADOW_CACHE_CONTROL", Cap::MmuShadowCacheControl),
            ("USER_MEMORY", Cap::UserMemory),
            ("SET_TSS_ADDR", Cap::SetTssAddr),
            ("VAPIC", Cap::Vapic),
            ("EXT_CPUID", Cap::ExtCpuid),
            ("CLOCKSOURCE", Cap::Clocksource),
            ("NR_VCPUS", Cap::NrVcpus),
            ("NR_MEMSLOTS", Cap::NrMemslots),
            ("PIT", Cap::Pit),
            ("PV_MMU", Cap::PvMmu),
            ("MP_STATE", Cap::MpState),
            ("COALESCED_MMIO", Cap::CoalescedMmio),
            ("SYNC_MMU", Cap::SyncMmu),
            ("IOMMU", Cap::Iommu),
            ("USER_NMI", Cap::UserNmi),
            ("IRQ_ROUTING", Cap::IrqRouting),
            ("IRQFD", Cap::Irqfd),
            ("PIT_STATE2", Cap::PitState2),
            ("IOEVENTFD", Cap::Ioeventfd),
            ("SET_IDENTITY_MAP_ADDR", Cap::SetIdentityMapAddr),
            ("ADJUST_CLOCK", Cap::AdjustClock),
            ("INTERNAL_ERROR_DATA", Cap::InternalErrorData),
            ("HYPERV", Cap::Hyperv),
            ("ENABLE_CAP", Cap::EnableCap),
            ("SYNC_REGS", Cap::SyncRegs),
            ("KVMCLOCK_CTRL", Cap::KvmclockCtrl),
            ("SIGNAL_MSI", Cap::SignalMsi),
            ("IOAPIC_POLARITY_IGNORED", Cap::IoapicPolarityIgnored),
            ("ENABLE_CAP_VM", Cap::EnableCapVm),
            ("IOEVENTFD_NO_LENGTH", Cap::IoeventfdNoLength),
            ("VM_ATTRIBUTES", Cap::VmAttributes),
            ("ARM_PSCI", Cap::ArmPsci),
            ("EXT_EMUL_CPUID", Cap::ExtEmulCpuid),
            ("CHECK_EXTENSION_VM", Cap::CheckExtensionVm),
            ("S390_USER_SIGP", Cap::S390UserSigp),
            ("MSI_DEVID", Cap::MsiDevid),
            ("PPC_GET_PVINFO", Cap::PpcGetPvinfo),
            ("PPC_BOOKE_WATCHDOG", Cap::PpcBookeWatchdog),
            ("PPC_EPR", Cap::PpcEpr),
            ("COALESCED_PIO", Cap::CoalescedPio),
            ("TSC_DEADLINE_TIMER", Cap::TscDeadlineTimer),
            ("ASYNC_PF", Cap::AsyncPf),
            ("PCI_2_3", Cap::Pci23),
            ("READONLY_MEM", Cap::ReadonlyMem),
            ("IRQFD_RESAMPLE", Cap::IrqfdResample),
            ("S390_GMAP", Cap::S390Gmap),
            ("PPC_GET_SMMU_INFO", Cap::PpcGetSmmuInfo),
            ("PPC_RTAS", Cap::PpcRtas),
            ("GET_MSR_FEATURES", Cap::GetMsrFeatures),
            ("MAX_VCPUS", Cap::MaxVcpus),
            ("MAX_VCPU_ID", Cap::MaxVcpuId),
            ("DEVICE_CTRL", Cap::DeviceCtrl),
            ("IMMEDIATE_EXIT", Cap::ImmediateExit),
            ("ARM_PMU_V3", Cap::ArmPmuV3),
        ];

        for &(name, cap) in probes {
            let available = kvm.check_extension(cap);
            flags.insert(name, available);
            if available {
                debug!("KVM capability available: {}", name);
            }
        }

        Self { flags }
    }

    /// Query a single capability by name.
    pub fn is_available(&self, name: &str) -> bool {
        self.flags.get(name).copied().unwrap_or(false)
    }

    /// Return a human-readable summary of all probed capabilities.
    pub fn summary(&self) -> String {
        let mut lines = Vec::new();
        lines.push("KVM capabilities:".to_string());

        let mut available = Vec::new();
        let mut unavailable = Vec::new();

        for (name, &present) in &self.flags {
            if present {
                available.push(*name);
            } else {
                unavailable.push(*name);
            }
        }

        available.sort();
        unavailable.sort();

        if !available.is_empty() {
            lines.push(format!("  ✓ {}", available.join(", ")));
        }
        if !unavailable.is_empty() {
            lines.push(format!("  ✗ {}", unavailable.join(", ")));
        }

        lines.join("\n")
    }
}

// ---------------------------------------------------------------------------
// Hypervisor
// ---------------------------------------------------------------------------

/// Thin wrapper around `/dev/kvm`.
pub struct Hypervisor {
    kvm: kvm_ioctls::Kvm,
    capabilities: KvmCapabilities,
}

impl std::fmt::Debug for Hypervisor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hypervisor")
            .field("api_version", &self.kvm.get_api_version())
            .finish()
    }
}

impl Hypervisor {
    /// Open `/dev/kvm` and probe host capabilities.
    pub fn new() -> Result<Self> {
        let kvm = kvm_ioctls::Kvm::new().map_err(|e| {
            let errno = e.errno();
            if errno == libc::ENOENT || errno == libc::ENODEV {
                LiteDroidError::KvmUnavailable(
                    "/dev/kvm not found — is the KVM kernel module loaded?".to_string(),
                )
            } else if errno == libc::EACCES {
                LiteDroidError::PermissionDenied(
                    "Permission denied opening /dev/kvm — try adding the user to the 'kvm' group."
                        .to_string(),
                )
            } else {
                LiteDroidError::KvmUnavailable(format!("failed to open /dev/kvm: {}", e))
            }
        })?;

        let capabilities = KvmCapabilities::new(&kvm);

        info!("KVM opened (API version {})", kvm.get_api_version());

        Ok(Self { kvm, capabilities })
    }

    /// Create a new KVM virtual machine.
    pub fn create_vm(&self) -> Result<Vm> {
        let vm_fd = self.kvm.create_vm().map_err(|e| {
            LiteDroidError::VmCreationFailed(format!("kvm create_vm failed: {}", e))
        })?;
        info!("KVM VM created");
        Ok(Vm { vm: vm_fd })
    }

    /// Return the KVM API version reported by the host.
    pub fn get_api_version(&self) -> i32 {
        self.kvm.get_api_version()
    }

    /// Query host-supported CPUID leaves.
    pub fn get_supported_cpuid(&self) -> Result<CpuId> {
        // Some KVM implementations, notably WSL2's KVM interface, reject a
        // large allocation even when the host supports a smaller CPUID set.
        // Retry with progressively smaller tables rather than failing VM
        // creation unnecessarily.
        let mut last_error = None;
        for num_entries in [256, 128, 64, 32] {
            match self.kvm.get_supported_cpuid(num_entries) {
                Ok(cpuid) => return Ok(cpuid),
                Err(error) => last_error = Some(error),
            }
        }
        Err(LiteDroidError::KvmIoctl(format!(
            "KVM_GET_SUPPORTED_CPUID failed after retries: {}",
            last_error.expect("CPUID retry list is non-empty")
        )))
    }

    /// Immutable reference to the capability table.
    pub fn capabilities(&self) -> &KvmCapabilities {
        &self.capabilities
    }

    /// Check a single KVM capability.
    pub fn check_extension(&self, cap: Cap) -> bool {
        self.kvm.check_extension(cap)
    }
}

// ---------------------------------------------------------------------------
// Vm
// ---------------------------------------------------------------------------

/// Wrapper around a KVM VM file-descriptor.
pub struct Vm {
    vm: kvm_ioctls::VmFd,
}

impl Vm {
    /// Create a new virtual CPU for this VM.
    pub fn create_vcpu(&self, index: u32) -> Result<Vcpu> {
        let vcpu_fd =
            self.vm
                .create_vcpu(index as u64)
                .map_err(|e| LiteDroidError::VcpuCreationFailed {
                    cpu_index: index,
                    reason: format!("kvm create_vcpu({}) failed: {}", index, e),
                })?;
        debug!("Created vCPU {}", index);
        Ok(Vcpu {
            vcpu: vcpu_fd,
            index,
        })
    }

    /// Create the standard x86 PIC/IOAPIC interrupt controller.
    pub fn create_irq_chip(&self) -> Result<()> {
        self.vm
            .create_irq_chip()
            .map_err(|e| LiteDroidError::KvmIoctl(format!("KVM_CREATE_IRQCHIP failed: {}", e)))
    }

    /// Create the standard x86 programmable interval timer.
    pub fn create_pit(&self) -> Result<()> {
        let config = unsafe { std::mem::zeroed::<kvm_bindings::kvm_pit_config>() };
        self.vm
            .create_pit2(config)
            .map_err(|e| LiteDroidError::KvmIoctl(format!("KVM_CREATE_PIT2 failed: {}", e)))
    }

    /// Set the Task-State Segment address (x86 only).
    pub fn set_tss_addr(&self, addr: usize) -> Result<()> {
        self.vm.set_tss_address(addr).map_err(|e| {
            LiteDroidError::KvmIoctl(format!("KVM_SET_TSS_ADDR({:#x}) failed: {}", addr, e))
        })
    }

    /// Set the identity-map page address (x86 only).
    pub fn set_identity_map_addr(&self, addr: u64) -> Result<()> {
        self.vm.set_identity_map_address(addr).map_err(|e| {
            LiteDroidError::KvmIoctl(format!(
                "KVM_SET_IDENTITY_MAP_ADDR({:#x}) failed: {}",
                addr, e
            ))
        })
    }

    /// Register a guest memory region with KVM.
    ///
    /// `slot` must be unique per VM. `guest_addr` is the guest-physical
    /// address, `memory_size` is the region length, and `userspace_addr`
    /// is the host-virtual pointer cast to `u64`.
    pub fn set_user_memory_region(
        &self,
        slot: u32,
        guest_addr: u64,
        memory_size: u64,
        userspace_addr: u64,
    ) -> Result<()> {
        let region = kvm_bindings::kvm_userspace_memory_region {
            slot,
            flags: 0,
            guest_phys_addr: guest_addr,
            memory_size,
            userspace_addr,
        };
        // SAFETY: The caller guarantees that `userspace_addr` points to
        // a valid, sufficiently large host allocation.
        unsafe {
            self.vm.set_user_memory_region(region).map_err(|e| {
                LiteDroidError::KvmIoctl(format!(
                    "KVM_SET_USER_MEMORY_REGION(slot={}, guest={:#x}, size={:#x}) failed: {}",
                    slot, guest_addr, memory_size, e
                ))
            })
        }
    }
}

// ---------------------------------------------------------------------------
// VcpuExitInfo (owned, no lifetime)
// ---------------------------------------------------------------------------

/// Owned representation of a KVM vCPU exit reason, safe to store and inspect
/// outside the immediate `Vcpu::run()` call.
#[derive(Debug, Clone)]
pub enum VcpuExitInfo {
    Hlt,
    IoOut { port: u16, data: Vec<u8> },
    IoIn { port: u16 },
    MmioRead { addr: u64, size: u8 },
    MmioWrite { addr: u64, size: u8, data: Vec<u8> },
    Exception,
    Shutdown,
    FailEntry { hardware_entry_failure_reason: u64 },
    InternalError,
    Unknown,
}

// ---------------------------------------------------------------------------
// Vcpu
// ---------------------------------------------------------------------------

/// Wrapper around a single KVM vCPU file-descriptor.
pub struct Vcpu {
    vcpu: kvm_ioctls::VcpuFd,
    index: u32,
}

impl Vcpu {
    /// Index of this vCPU (0-based).
    pub fn index(&self) -> u32 {
        self.index
    }

    /// Read general-purpose registers.
    pub fn get_regs(&self) -> Result<kvm_bindings::kvm_regs> {
        self.vcpu
            .get_regs()
            .map_err(|e| LiteDroidError::VcpuRunFailed {
                cpu_index: self.index,
                reason: format!("GET_REGS failed: {}", e),
            })
    }

    /// Write general-purpose registers.
    pub fn set_regs(&self, regs: &kvm_bindings::kvm_regs) -> Result<()> {
        self.vcpu
            .set_regs(regs)
            .map_err(|e| LiteDroidError::VcpuRunFailed {
                cpu_index: self.index,
                reason: format!("SET_REGS failed: {}", e),
            })
    }

    /// Read special / system registers.
    pub fn get_sregs(&self) -> Result<kvm_bindings::kvm_sregs> {
        self.vcpu
            .get_sregs()
            .map_err(|e| LiteDroidError::VcpuRunFailed {
                cpu_index: self.index,
                reason: format!("GET_SREGS failed: {}", e),
            })
    }

    /// Write special / system registers.
    pub fn set_sregs(&self, sregs: &kvm_bindings::kvm_sregs) -> Result<()> {
        self.vcpu
            .set_sregs(sregs)
            .map_err(|e| LiteDroidError::VcpuRunFailed {
                cpu_index: self.index,
                reason: format!("SET_SREGS failed: {}", e),
            })
    }

    /// Program the CPUID leaves for this vCPU.
    pub fn set_cpuid(&self, cpuid: &CpuId) -> Result<()> {
        self.vcpu
            .set_cpuid2(cpuid)
            .map_err(|e| LiteDroidError::VcpuRunFailed {
                cpu_index: self.index,
                reason: format!("SET_CPUID failed: {}", e),
            })
    }

    /// Run the vCPU until it exits. Returns an owned summary of the exit reason.
    pub fn run(&mut self) -> Result<VcpuExitInfo> {
        self.run_with_mmio(|_, _| {})
    }

    /// Run the vCPU and complete MMIO reads before returning to the caller.
    pub fn run_with_mmio<F>(&mut self, mut mmio_read: F) -> Result<VcpuExitInfo>
    where
        F: FnMut(u64, &mut [u8]),
    {
        let exit = self.vcpu.run().map_err(|e| LiteDroidError::VcpuRunFailed {
            cpu_index: self.index,
            reason: format!("KVM_RUN failed: {}", e),
        })?;

        Ok(match exit {
            VcpuExit::Hlt => VcpuExitInfo::Hlt,
            VcpuExit::IoOut(port, data) => VcpuExitInfo::IoOut {
                port,
                data: data.to_vec(),
            },
            VcpuExit::IoIn(port, _data) => VcpuExitInfo::IoIn { port },
            VcpuExit::MmioRead(addr, data) => {
                mmio_read(addr, data);
                VcpuExitInfo::MmioRead {
                    addr,
                    size: data.len() as u8,
                }
            }
            VcpuExit::MmioWrite(addr, data) => VcpuExitInfo::MmioWrite {
                addr,
                size: data.len() as u8,
                data: data.to_vec(),
            },
            VcpuExit::Exception => VcpuExitInfo::Exception,
            VcpuExit::Shutdown => VcpuExitInfo::Shutdown,
            VcpuExit::FailEntry(reason, _cpu) => VcpuExitInfo::FailEntry {
                hardware_entry_failure_reason: reason,
            },
            VcpuExit::InternalError => VcpuExitInfo::InternalError,
            VcpuExit::Unknown => {
                warn!("vCPU {}: unknown exit reason", self.index);
                VcpuExitInfo::Unknown
            }
            // All other exit reasons are mapped to Unknown for now.
            _ => {
                trace!("vCPU {}: unhandled exit reason", self.index);
                VcpuExitInfo::Unknown
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Re-export widely used types for downstream convenience
// ---------------------------------------------------------------------------

#[allow(unused_imports)]
pub use kvm_bindings::{kvm_regs, kvm_sregs};
