//! Virtual Machine Monitor — wires together KVM, guest memory and boot setup
//! into a runnable virtual machine.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use kvm_bindings::kvm_segment;
use litedroid_core::constants::*;
use litedroid_core::device::DeviceConfig;
use litedroid_core::error::{LiteDroidError, Result};
use litedroid_kvm::cpuid::filter_for_android;
use litedroid_kvm::{Hypervisor, Vcpu, VcpuExitInfo, Vm};
use litedroid_memory::GuestMemory;
use litedroid_storage::RawDiskImage;
use tracing::{debug, error, info, trace};

const VIRTIO_MMIO_START: u64 = 0xfea0_0000;
const VIRTIO_MMIO_SIZE: u64 = 0x200;
const VIRTIO_MAGIC: u32 = 0x7472_6976;
const VIRTIO_BLK: u32 = 2;
const DESC_NEXT: u16 = 1;
const DESC_WRITE: u16 = 2;

// ---------------------------------------------------------------------------
// Boot-layout constants (guest-physical addresses)
// ---------------------------------------------------------------------------

/// Address of the top-level page-map level-4 table.
const PML4_ADDR: u64 = 0x70_000;

/// Address of the page-directory-pointer table.
const PDPT_ADDR: u64 = 0x71_000;

/// Address of the page-directory (2 MB pages).
const PD_ADDR: u64 = 0x72_000;

/// Address of the boot GDT.
const GDT_ADDR: u64 = 0x80_000;

/// GDT selector for the 64-bit code segment (entry 1 × 8).
const KERNEL_CODE_SEGMENT: u16 = 0x10;

/// GDT selector for the data segment (entry 2 × 8).
const KERNEL_DATA_SEGMENT: u16 = 0x18;

/// Initial RSP — stack top, placed just below the initramfs area.
const BOOT_STACK_POINTER: u64 = 0x800_000;

/// Linux x86 boot protocol zero page and command-line locations.
const BOOT_PARAMS_ADDR: u64 = 0x90_000;
const CMDLINE_ADDR: u64 = 0x20_000;
const BOOT_PARAMS_SIZE: usize = 0x1000;

// ---------------------------------------------------------------------------
// VirtualMachine
// ---------------------------------------------------------------------------

/// Top-level object that owns all resources needed to run a single guest.
#[allow(dead_code)]
pub struct VirtualMachine {
    /// KVM hypervisor handle (kept alive for capability queries).
    hypervisor: Hypervisor,
    /// KVM VM file descriptor.
    vm: Vm,
    /// One `Vcpu` per `config.vcpu_count`.
    vcpus: Vec<Vcpu>,
    /// Contiguous guest-physical memory region.
    guest_memory: GuestMemory,
    /// Frozen device configuration snapshot.
    config: DeviceConfig,
    kernel_is_bzimage: bool,
    bzimage_header: Vec<u8>,
    initramfs_size: u64,
    block_device: VirtioBlock,
    /// Shared flag — set to `false` to request the run-loop to stop.
    running: Arc<AtomicBool>,
}

impl VirtualMachine {
    /// Build a complete virtual machine from the given configuration.
    ///
    /// This opens KVM, creates a VM, maps guest RAM, registers it with KVM,
    /// and creates the requested number of vCPUs.
    pub fn new(config: &DeviceConfig) -> Result<Self> {
        config.validate()?;

        let hypervisor = Hypervisor::new()?;
        let vm = hypervisor.create_vm()?;

        // Android's Linux kernel expects the conventional x86 interrupt
        // controller to exist before it initializes its interrupt tables.
        vm.create_irq_chip()?;
        vm.create_pit()?;

        // --- x86 one-time setup ---
        vm.set_tss_addr(0xfffb_d000)?;
        vm.set_identity_map_addr(0xfffbc000)?;

        // --- Guest memory ---
        let mem_size = config.ram_mb * 1024 * 1024;
        let guest_memory = GuestMemory::new(mem_size)?;

        vm.set_user_memory_region(
            0, // slot
            0, // guest_phys_addr
            mem_size,
            guest_memory.userspace_addr(),
        )?;

        // --- vCPUs ---
        let mut vcpus = Vec::with_capacity(config.vcpu_count as usize);
        let cpuid = hypervisor.get_supported_cpuid()?;
        let mut filtered_cpuid = cpuid;
        filter_for_android(&mut filtered_cpuid);

        for i in 0..config.vcpu_count {
            let vcpu = vm.create_vcpu(i)?;
            vcpu.set_cpuid(&filtered_cpuid)?;
            vcpus.push(vcpu);
        }

        info!(
            "VirtualMachine created: {} vCPUs, {} MB RAM",
            config.vcpu_count, config.ram_mb
        );

        Ok(Self {
            hypervisor,
            vm,
            vcpus,
            guest_memory,
            config: config.clone(),
            kernel_is_bzimage: false,
            bzimage_header: Vec::new(),
            initramfs_size: 0,
            block_device: VirtioBlock::new(&config.system_image_path)?,
            running: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Load a Linux kernel binary (bzImage / Image) into guest memory.
    pub fn load_kernel(&mut self, path: &std::path::Path) -> Result<()> {
        if !path.exists() {
            return Err(LiteDroidError::KernelNotFound(path.to_path_buf()));
        }
        let data = std::fs::read(path).map_err(|e| {
            LiteDroidError::KernelLoadFailed(format!("reading {}: {}", path.display(), e))
        })?;
        if data.len() >= 0x206 && &data[0x202..0x206] == b"HdrS" {
            let setup_sects = data[0x1f1] as usize;
            let setup_size = (setup_sects + 1).checked_mul(512).ok_or_else(|| {
                LiteDroidError::KernelLoadFailed("bzImage setup size overflowed".to_string())
            })?;
            let payload_offset = read_u32(&data, 0x248)? as usize;
            let payload_length = read_u32(&data, 0x24c)? as usize;
            let payload_end = data.len();
            if setup_size < 0x200 || payload_offset < 0x200 || setup_size >= payload_end {
                return Err(LiteDroidError::KernelLoadFailed(format!(
                    "invalid bzImage payload range: setup={setup_size:#x}, header offset={payload_offset:#x}, length={payload_length:#x}"
                )));
            }
            // The setup sectors contain the real-mode boot protocol data and
            // the protected-mode entry begins immediately after them.
            self.guest_memory.load_blob(0x10000, &data[..setup_size])?;
            info!("Loading bzImage protected payload: file offset={:#x}, file size={:#x} (header length {:#x}), load addr={:#x}", setup_size, payload_end - setup_size, payload_length, KERNEL_LOAD_ADDR);
            self.guest_memory
                .load_blob(KERNEL_LOAD_ADDR, &data[setup_size..payload_end])?;
            // Verify kernel was loaded correctly
            let mut verify_buf = [0u8; 16];
            self.guest_memory
                .read_guest(KERNEL_LOAD_ADDR, &mut verify_buf)
                .ok();
            info!(
                "Kernel loaded, first 16 bytes at {:#x}: {:02x?}",
                KERNEL_LOAD_ADDR, &verify_buf
            );

            self.kernel_is_bzimage = true;
            self.bzimage_header = data[0x1f1..0x290].to_vec();
            return Ok(());
        }
        info!(
            "Loading kernel ({} bytes) at {:#x}",
            data.len(),
            KERNEL_LOAD_ADDR
        );
        self.guest_memory.load_blob(KERNEL_LOAD_ADDR, &data)?;
        Ok(())
    }

    /// Load an initramfs / ramdisk into guest memory.
    pub fn load_initramfs(&mut self, path: &std::path::Path) -> Result<()> {
        if !path.exists() {
            return Err(LiteDroidError::InitramfsNotFound(path.to_path_buf()));
        }
        let data = std::fs::read(path).map_err(|e| {
            LiteDroidError::KernelLoadFailed(format!("reading initramfs {}: {}", path.display(), e))
        })?;
        info!(
            "Loading initramfs ({} bytes) at {:#x}",
            data.len(),
            INITRAMFS_LOAD_ADDR
        );
        self.guest_memory.load_blob(INITRAMFS_LOAD_ADDR, &data)?;
        self.initramfs_size = data.len() as u64;
        Ok(())
    }

    /// Programme the identity-mapped page tables, GDT, and system registers
    /// on vCPU 0 so the guest kernel can start executing in 64-bit long mode.
    pub fn setup_boot(&self) -> Result<()> {
        let vcpu = &self.vcpus[0];

        if self.kernel_is_bzimage {
            return self.setup_bzimage_boot(vcpu);
        }

        // ---- Identity-mapped page tables (2 MB pages) ----
        self.setup_page_tables()?;

        // ---- GDT ----
        self.setup_gdt()?;

        // ---- System registers ----
        self.setup_sregs(vcpu)?;

        // ---- General-purpose registers ----
        let mut regs = vcpu.get_regs()?;
        regs.rip = KERNEL_LOAD_ADDR;
        regs.rsi = INITRAMFS_LOAD_ADDR; // ptr to initrd / FDT in RDI on x86-64 actually
                                        // NOTE: kernel typically receives initrd addr in RSI for x86-64 boot protocol
        regs.rsp = BOOT_STACK_POINTER;
        vcpu.set_regs(&regs)?;

        debug!(
            "Boot setup complete: RIP={:#x}, RSI={:#x}, RSP={:#x}",
            regs.rip, regs.rsi, regs.rsp
        );

        Ok(())
    }

    fn setup_bzimage_boot(&self, vcpu: &Vcpu) -> Result<()> {
        let mut boot_params = vec![0u8; BOOT_PARAMS_SIZE];
        if self.bzimage_header.len() != 0x9f {
            return Err(LiteDroidError::KernelLoadFailed(
                "bzImage boot header was not retained".to_string(),
            ));
        }
        boot_params[0x1f1..0x290].copy_from_slice(&self.bzimage_header);

        // Place the command line in its own buffer. The zero-page field at
        // 0x228 is a pointer, not inline command-line storage.
        let cmdline = self.config.kernel_cmdline.as_bytes();
        if cmdline.len() + 1 > 0x800 {
            return Err(LiteDroidError::KernelLoadFailed(
                "kernel command line exceeds boot parameter space".to_string(),
            ));
        }
        let mut cmdline_data = vec![0u8; cmdline.len() + 1];
        cmdline_data[..cmdline.len()].copy_from_slice(cmdline);
        self.guest_memory.load_blob(CMDLINE_ADDR, &cmdline_data)?;
        write_u32(&mut boot_params, 0x228, CMDLINE_ADDR as u32);
        write_u32(&mut boot_params, 0x238, cmdline.len() as u32);

        // Linux boot protocol fields for the initramfs.
        write_u32(&mut boot_params, 0x218, INITRAMFS_LOAD_ADDR as u32);
        write_u32(&mut boot_params, 0x21c, self.initramfs_size as u32);
        write_u16(&mut boot_params, 0x224, 0x7fff); // heap end, in 16-byte units

        // Set boot loader fields
        boot_params[0x210] = 0xff; // type_of_loader: custom loader
        boot_params[0x211] |= 0x80; // CAN_USE_HEAP

        // Load boot parameters (zero-page) at 0x90000 per Linux boot protocol
        self.guest_memory
            .load_blob(BOOT_PARAMS_ADDR, &boot_params)?;
        info!(
            "Boot parameters loaded at {:#x}, command line: {}",
            BOOT_PARAMS_ADDR,
            String::from_utf8_lossy(cmdline)
        );

        // The bzImage decompressor runs in 32-bit protected mode.
        // It expects:
        // - ESI: pointer to boot params (zero page)
        // - Flat memory addressing (no paging) for initial decompression
        // - Enough memory for decompression

        // Set up 32-bit GDT (required for protected mode)
        self.setup_gdt_32bit()?;

        // Set up system registers for 32-bit protected mode
        self.setup_sregs_32bit(vcpu)?; // Set up system registers for 32-bit protected mode (flat memory)

        // Set general-purpose registers
        let mut regs = vcpu.get_regs()?;
        regs.rax = 0;
        regs.rbx = 0;
        regs.rcx = 0;
        regs.rdx = 0;
        regs.rsi = BOOT_PARAMS_ADDR; // ESI = pointer to boot params
        regs.rdi = 0;
        regs.rbp = 0;
        regs.rip = KERNEL_LOAD_ADDR; // Entry at start of kernel payload (0x100000)
        regs.rsp = 0x10000; // Stack at 64KB (should be below kernel)
        regs.rflags = 0x2; // standard flags
        vcpu.set_regs(&regs)?;

        info!(
            "bzImage 32-bit boot ready: RIP={:#x}, RSI={:#x} (boot params), RSP={:#x}",
            regs.rip, regs.rsi, regs.rsp
        );
        Ok(())
    }
    /// The bzImage decompressor requires 32-bit protected mode, not 64-bit long mode.
    fn setup_gdt_32bit(&self) -> Result<()> {
        // Linux expects code selector 0x10 and data selector 0x18 on entry.
        let mut gdt = [0u8; 32];

        // Entry 0: Null descriptor
        // (already zeros)

        // Entry 2 at offset 0x10: Code segment - 32-bit execute/read
        // Base=0, Limit=0xFFFFFFFF, Flags=0x9a (P=1, DPL=0, S=1, Type=0xa)
        // DB=1 (32-bit), G=1 (4KB granularity)
        let code_seg: u64 = 0x00cf9a00_0000ffff;
        gdt[0x10..0x18].copy_from_slice(&code_seg.to_le_bytes());

        // Entry 3 at offset 0x18: Data segment - read/write
        // Base=0, Limit=0xFFFFFFFF, Flags=0x92 (P=1, DPL=0, S=1, Type=0x2)
        // DB=1 (32-bit), G=1 (4KB granularity)
        let data_seg: u64 = 0x00cf9200_0000ffff;
        gdt[0x18..0x20].copy_from_slice(&data_seg.to_le_bytes());

        self.guest_memory.load_blob(GDT_ADDR, &gdt)?;
        info!(
            "32-bit GDT set up at {:#x}: code={:#x} data={:#x}",
            GDT_ADDR, code_seg, data_seg
        );
        Ok(())
    }

    /// Set up system registers for 32-bit protected mode (bzImage decompression).
    fn setup_sregs_32bit(&self, vcpu: &Vcpu) -> Result<()> {
        let mut sregs = vcpu.get_sregs()?;

        // Set GDT
        sregs.gdt.base = GDT_ADDR;
        sregs.gdt.limit = 31; // 4 entries × 8 - 1

        // Set code segment (selector 0x10) for 32-bit protected mode
        sregs.cs = kvm_bindings::kvm_segment {
            selector: 0x10,
            base: 0,
            limit: 0xffffffff,
            type_: 0xa, // code, readable
            present: 1,
            dpl: 0,
            db: 1, // 32-bit
            s: 1,
            l: 0, // not 64-bit
            g: 1, // 4KB granularity
            avl: 0,
            ..unsafe { std::mem::zeroed() }
        };

        // Set data segment (selector 0x18) for 32-bit protected mode
        sregs.ds = kvm_bindings::kvm_segment {
            selector: 0x18,
            base: 0,
            limit: 0xffffffff,
            type_: 0x2, // data, writable
            present: 1,
            dpl: 0,
            db: 1, // 32-bit
            s: 1,
            l: 0,
            g: 1,
            avl: 0,
            ..unsafe { std::mem::zeroed() }
        };

        // Copy data segment to all other data/stack segments
        sregs.ss = sregs.ds;
        sregs.es = sregs.ds;
        sregs.fs = sregs.ds;
        sregs.gs = sregs.ds;

        // 32-bit protected mode without paging (flat memory)
        // Linux x86 decompressor typically runs in flat memory addressing
        // for initial decompression. The kernel sets up paging after decompression.
        sregs.cr0 = 0x00000011; // PE | ET, no PG
        sregs.cr3 = 0; // Not used without paging
        sregs.cr4 = 0x00000000; // Standard 32-bit, no PAE
        sregs.efer = 0; // no long mode

        vcpu.set_sregs(&sregs)?;
        info!("System registers configured for 32-bit protected mode (flat memory): CR0={:#x}, CR4={:#x}", 
               sregs.cr0, sregs.cr4);
        Ok(())
    }
    /// - PML4  @ `0x70_000`  — 1 entry pointing to PDPT
    /// - PDPT  @ `0x71_000`  — 1 entry pointing to PD
    /// - PD    @ `0x72_000`  — N entries, each mapping a 2 MB page
    fn setup_page_tables(&self) -> Result<()> {
        let mem_size = self.config.ram_mb * 1024 * 1024;
        let num_pages = (mem_size + (2 * 1024 * 1024) - 1) / (2 * 1024 * 1024); // round up

        // --- PML4 (entry 0 → PDPT) ---
        // flags: P (1) | RW (2) | NX (0) = 3
        let pml4_entry: u64 = PDPT_ADDR | 0x03;
        self.guest_memory
            .write_guest(PML4_ADDR, &pml4_entry.to_le_bytes())?;

        // --- PDPT (entry 0 → PD) ---
        let pdpt_entry: u64 = PD_ADDR | 0x03;
        self.guest_memory
            .write_guest(PDPT_ADDR, &pdpt_entry.to_le_bytes())?;

        // --- PD (N entries, 2 MB pages, PS bit = 1) ---
        // flags: P (1) | RW (2) | PS (0x80) | NX (0x8000000000000000) = 0x83
        // Note: Ensure all kernel regions are executable (NX=0)
        for i in 0..num_pages.min(512) {
            let addr = (i as u64) * 2 * 1024 * 1024;
            // P (1) | RW (2) | PS (0x80) = 0x83, NX bit (63) = 0 for executable
            let pd_entry: u64 = addr | 0x83;
            let offset = PD_ADDR + (i as u64) * 8;
            self.guest_memory
                .write_guest(offset, &pd_entry.to_le_bytes())?;
        }

        info!(
            "Page tables set up: PML4={:#x}, PDPT={:#x}, PD={:#x}, {} 2MB entries covering {:.1} MB",
            PML4_ADDR, PDPT_ADDR, PD_ADDR, num_pages, mem_size as f64 / (1024.0 * 1024.0)
        );
        Ok(())
    }

    /// Write a minimal GDT (null + 64-bit code + data) at `GDT_ADDR`.
    fn setup_gdt(&self) -> Result<()> {
        // Null descriptor
        self.guest_memory
            .write_guest(GDT_ADDR, &0u64.to_le_bytes())?;
        // 64-bit code segment (L=1, D=0, P=1, S=1, Type=execute/read, accessed)
        let code_seg: u64 = 0x00af9b0000000000;
        self.guest_memory
            .write_guest(GDT_ADDR + 8, &0u64.to_le_bytes())?;
        self.guest_memory
            .write_guest(GDT_ADDR + 16, &code_seg.to_le_bytes())?;
        // Data segment (RW, P=1, S=1, D/B=1, G=1)
        let data_seg: u64 = 0x00cf93000000ffff;
        self.guest_memory
            .write_guest(GDT_ADDR + 24, &data_seg.to_le_bytes())?;

        debug!("GDT written at {:#x}", GDT_ADDR);
        Ok(())
    }

    /// Configure the system registers on vCPU 0 so that the processor enters
    /// 64-bit long mode.
    fn setup_sregs(&self, vcpu: &Vcpu) -> Result<()> {
        let mut sregs = vcpu.get_sregs()?;

        // ---- CR0: PE + ET + PG ----
        sregs.cr0 = 0x80000011;

        // ---- CR4: PAE (bit 5) ----
        sregs.cr4 = 0x20;

        // ---- EFER: LME (bit 8) + LMA (bit 10) ----
        sregs.efer = 0x500;

        // ---- Segment selectors ----
        sregs.cs = kvm_segment {
            selector: KERNEL_CODE_SEGMENT,
            base: 0,
            limit: 0xfffff,
            // Present, L (long mode), D (default=0 for 64-bit), G (granularity 4K),
            // Type = execute/read (0xa), DPL = 0, S = 1
            type_: 0xb,
            present: 1,
            dpl: 0,
            db: 0,
            s: 1,
            l: 1,
            g: 1,
            avl: 0,
            ..unsafe { std::mem::zeroed() }
        };

        sregs.ds = make_flat_data_segment(KERNEL_DATA_SEGMENT);
        sregs.es = make_flat_data_segment(KERNEL_DATA_SEGMENT);
        sregs.fs = make_flat_data_segment(KERNEL_DATA_SEGMENT);
        sregs.gs = make_flat_data_segment(KERNEL_DATA_SEGMENT);
        sregs.ss = make_flat_data_segment(KERNEL_DATA_SEGMENT);

        // Load the GDT that was written into guest memory above. Without a
        // valid GDTR, KVM rejects the non-null segment selectors.
        sregs.gdt.base = GDT_ADDR;
        sregs.gdt.limit = (4 * std::mem::size_of::<u64>() - 1) as u16;

        // ---- Page tables ----
        sregs.cr3 = PML4_ADDR;

        vcpu.set_sregs(&sregs)?;

        debug!("System registers configured for 64-bit long mode");
        Ok(())
    }

    /// Run the guest until it halts, shuts down, or is stopped externally.
    ///
    /// Currently only vCPU 0 is driven; a future implementation will spawn
    /// one thread per vCPU.
    pub fn run(&mut self) -> Result<()> {
        self.running.store(true, Ordering::SeqCst);
        info!("VM run loop started");

        // Only drive vCPU 0 for now.
        let vcpu = &mut self.vcpus[0];
        let guest_memory = &self.guest_memory;
        let block_device = &mut self.block_device;

        while self.running.load(Ordering::SeqCst) {
            match vcpu.run_with_mmio(|addr, data| {
                if (VIRTIO_MMIO_START..VIRTIO_MMIO_START + VIRTIO_MMIO_SIZE).contains(&addr) {
                    let value = block_device.read(addr - VIRTIO_MMIO_START, data.len());
                    let bytes = value.to_le_bytes();
                    data.copy_from_slice(&bytes[..data.len()]);
                }
            }) {
                Ok(exit) => match exit {
                    VcpuExitInfo::Hlt | VcpuExitInfo::Shutdown => {
                        let regs = vcpu.get_regs().unwrap_or_default();
                        info!(
                            "Guest HLT/Shutdown at RIP={:#x}: RAX={:#x} RBX={:#x} RCX={:#x} RDX={:#x} RSI={:#x} RDI={:#x} RSP={:#x}",
                            regs.rip, regs.rax, regs.rbx, regs.rcx, regs.rdx, regs.rsi, regs.rdi, regs.rsp
                        );
                        // Attempt to read kernel code around halt point.
                        let mut code_buf = [0u8; 16];
                        if self
                            .guest_memory
                            .read_guest(regs.rip, &mut code_buf)
                            .is_ok()
                        {
                            info!("Code bytes at halt: {:02x?}", &code_buf);
                        }
                        self.running.store(false, Ordering::SeqCst);
                    }
                    VcpuExitInfo::IoOut { port, data } => {
                        if port == SERIAL_IO_BASE {
                            // Capture kernel serial output to logs.
                            for &byte in &data {
                                if byte >= 32 && byte < 127 {
                                    info!("[GUEST SERIAL] {}", byte as char);
                                } else if byte == 10 {
                                    info!("[GUEST SERIAL] <LF>");
                                } else if byte == 13 {
                                    info!("[GUEST SERIAL] <CR>");
                                } else {
                                    info!("[GUEST SERIAL] <0x{:02x}>", byte);
                                }
                            }
                        } else {
                            trace!("IO write: port={:#x}, data={:?}", port, data);
                        }
                    }
                    VcpuExitInfo::IoIn { port } => {
                        trace!("IO read: port={:#x} (returning 0xff)", port);
                        // KVM handles filling the return buffer; we simply
                        // continue.  The guest will see whatever was in the
                        // buffer (0xff by default for unhandled IO).
                    }
                    VcpuExitInfo::MmioRead { addr, size } => {
                        trace!("MMIO read: addr={:#x}, size={}", addr, size);
                    }
                    VcpuExitInfo::MmioWrite { addr, size, data } => {
                        trace!(
                            "MMIO write: addr={:#x}, size={}, data={:?}",
                            addr,
                            size,
                            data
                        );
                        if (VIRTIO_MMIO_START..VIRTIO_MMIO_START + VIRTIO_MMIO_SIZE).contains(&addr)
                        {
                            block_device.write(
                                addr - VIRTIO_MMIO_START,
                                size,
                                u64::from_le_bytes({
                                    let mut value = [0u8; 8];
                                    value[..data.len()].copy_from_slice(&data);
                                    value
                                }),
                                guest_memory,
                            );
                        }
                    }
                    VcpuExitInfo::Exception => {
                        let regs = vcpu.get_regs().unwrap_or_default();
                        let sregs = vcpu.get_sregs().ok();
                        error!(
                            "Guest exception at RIP={:#x}: RAX={:#x} RSI={:#x} RSP={:#x}, CS={:#x}, DS={:#x}",
                            regs.rip,
                            regs.rax,
                            regs.rsi,
                            regs.rsp,
                            sregs.as_ref().map(|value| value.cs.selector).unwrap_or_default(),
                            sregs.as_ref().map(|value| value.ds.selector).unwrap_or_default(),
                        );
                        self.running.store(false, Ordering::SeqCst);
                    }
                    VcpuExitInfo::FailEntry {
                        hardware_entry_failure_reason,
                    } => {
                        let regs = vcpu.get_regs().unwrap_or_default();
                        error!(
                            "vCPU fail entry at RIP={:#x}: reason={:#x} (may indicate invalid CPU state, registers: RAX={:#x} RBX={:#x} RCX={:#x})",
                            regs.rip, hardware_entry_failure_reason, regs.rax, regs.rbx, regs.rcx
                        );
                        self.running.store(false, Ordering::SeqCst);
                    }
                    VcpuExitInfo::InternalError => {
                        let regs = vcpu.get_regs().unwrap_or_default();
                        error!("vCPU internal error at RIP={:#x} — stopping VM", regs.rip);
                        self.running.store(false, Ordering::SeqCst);
                    }
                    VcpuExitInfo::Unknown => {
                        let regs = vcpu.get_regs().ok();
                        if let Some(r) = regs {
                            trace!("vCPU unknown exit at RIP={:#x}", r.rip);
                        } else {
                            trace!("vCPU unknown exit");
                        }
                    }
                },
                Err(e) => {
                    error!("vCPU run error: {}", e);
                    self.running.store(false, Ordering::SeqCst);
                }
            }
        }

        info!("VM run loop exited");
        Ok(())
    }

    /// Request the run-loop to stop on the next iteration.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        debug!("VM stop requested");
    }

    /// Return a shared stop flag for a VM run loop owned by another thread.
    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        self.running.clone()
    }

    /// Returns `true` while the run-loop is active.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Borrow the guest memory region.
    pub fn guest_memory(&self) -> &GuestMemory {
        &self.guest_memory
    }

    /// Borrow the device configuration.
    pub fn config(&self) -> &DeviceConfig {
        &self.config
    }
}

struct VirtioBlock {
    disk: RawDiskImage,
    queue_num: u16,
    queue_pfn: u32,
    status: u32,
    last_avail: u16,
}

impl VirtioBlock {
    fn new(path: &std::path::Path) -> Result<Self> {
        Ok(Self {
            disk: RawDiskImage::open(path)?,
            queue_num: 128,
            queue_pfn: 0,
            status: 0,
            last_avail: 0,
        })
    }

    fn read(&self, offset: u64, size: usize) -> u64 {
        let value = match offset {
            0x00 => VIRTIO_MAGIC as u64,
            0x04 => 2,
            0x08 => VIRTIO_BLK as u64,
            0x0c => 0,
            0x10 => 0,
            0x34 => self.queue_num as u64,
            0x38 => self.queue_num as u64,
            0x3c => 4096,
            0x40 => self.queue_pfn as u64,
            0x60 => self.status as u64,
            0x64 => 0,
            0x100 => self.disk.sectors(),
            _ => 0,
        };
        if size >= 8 {
            value
        } else {
            value & ((1u64 << (size * 8)) - 1)
        }
    }

    fn write(&mut self, offset: u64, _size: u8, value: u64, memory: &GuestMemory) {
        match offset {
            0x38 => self.queue_num = (value as u16).min(128),
            0x40 => self.queue_pfn = value as u32,
            0x50 => {
                if value == 0 && self.queue_pfn != 0 {
                    if let Err(error) = self.process_queue(memory) {
                        error!(%error, "virtio-blk request failed");
                    }
                }
            }
            0x60 => self.status = value as u32,
            _ => {}
        }
    }

    fn process_queue(&mut self, memory: &GuestMemory) -> Result<()> {
        let queue = (self.queue_pfn as u64) << 12;
        let avail_idx = guest_u16(memory, queue + 2)?;
        while self.last_avail != avail_idx {
            let slot = (self.last_avail % self.queue_num) as u64;
            let head = guest_u16(memory, queue + 4 + slot * 2)?;
            self.process_request(memory, queue, head)?;
            self.last_avail = self.last_avail.wrapping_add(1);
        }
        Ok(())
    }

    fn process_request(&mut self, memory: &GuestMemory, queue: u64, head: u16) -> Result<()> {
        let desc = queue + (head as u64) * 16;
        let header_addr = guest_u64(memory, desc)?;
        let header_flags = guest_u16(memory, desc + 12)?;
        let header_next = guest_u16(memory, desc + 14)?;
        if header_flags & DESC_NEXT == 0 {
            return Err(LiteDroidError::DiskIo(
                "virtio request has no data descriptor".to_string(),
            ));
        }
        let request_type = guest_u32(memory, header_addr)?;
        let sector = guest_u64(memory, header_addr + 8)?;
        let payload_desc = queue + (header_next as u64) * 16;
        let payload_addr = guest_u64(memory, payload_desc)?;
        let payload_len = guest_u32(memory, payload_desc + 8)? as usize;
        let payload_flags = guest_u16(memory, payload_desc + 12)?;
        let status_next = guest_u16(memory, payload_desc + 14)?;
        if payload_flags & DESC_NEXT == 0 || status_next as u64 >= self.queue_num as u64 {
            return Err(LiteDroidError::DiskIo(
                "virtio request has invalid status descriptor".to_string(),
            ));
        }
        let status_desc = queue + (status_next as u64) * 16;
        let status_addr = guest_u64(memory, status_desc)?;
        if status_addr == 0 {
            return Err(LiteDroidError::DiskIo(
                "virtio status address is null".to_string(),
            ));
        }

        let mut payload = vec![0u8; payload_len];
        let status = match request_type {
            0 => {
                if payload_flags & DESC_WRITE == 0 {
                    return Err(LiteDroidError::DiskIo(
                        "virtio read buffer is not writable".to_string(),
                    ));
                }
                let bytes = self.disk.read(sector * 512, &mut payload)?;
                memory.write_guest(payload_addr, &payload[..bytes])?;
                0u8
            }
            1 => {
                if payload_flags & DESC_WRITE != 0 {
                    return Err(LiteDroidError::DiskIo(
                        "virtio write buffer is not readable".to_string(),
                    ));
                }
                memory.read_guest(payload_addr, &mut payload)?;
                self.disk.write(sector * 512, &payload)?;
                0u8
            }
            _ => 1u8,
        };
        memory.write_guest(status_addr, &[status])?;

        let used = align_up(
            queue + 16 * self.queue_num as u64 + 4 + 2 * self.queue_num as u64,
            4096,
        );
        let used_idx = guest_u16(memory, used + 2)?;
        let used_slot = (used_idx % self.queue_num) as u64;
        memory.write_guest(used + 4 + used_slot * 8, &(head as u32).to_le_bytes())?;
        memory.write_guest(
            used + 8 + used_slot * 8,
            &(payload_len as u32).to_le_bytes(),
        )?;
        memory.write_guest(used + 2, &used_idx.wrapping_add(1).to_le_bytes())?;
        Ok(())
    }
}

fn align_up(value: u64, alignment: u64) -> u64 {
    (value + alignment - 1) & !(alignment - 1)
}

fn guest_u16(memory: &GuestMemory, address: u64) -> Result<u16> {
    let mut bytes = [0u8; 2];
    memory.read_guest(address, &mut bytes)?;
    Ok(u16::from_le_bytes(bytes))
}

fn guest_u32(memory: &GuestMemory, address: u64) -> Result<u32> {
    let mut bytes = [0u8; 4];
    memory.read_guest(address, &mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn guest_u64(memory: &GuestMemory, address: u64) -> Result<u64> {
    let mut bytes = [0u8; 8];
    memory.read_guest(address, &mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a flat 32/64-bit data segment descriptor.
fn make_flat_data_segment(selector: u16) -> kvm_bindings::kvm_segment {
    kvm_bindings::kvm_segment {
        selector,
        base: 0,
        limit: 0xfffff,
        // Type = accessed/read/write data segment, present, S=1
        type_: 0x3,
        present: 1,
        dpl: 0,
        db: 1, // default 1 for data segments
        s: 1,
        l: 0,
        g: 1,
        avl: 0,
        ..unsafe { std::mem::zeroed() }
    }
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32> {
    let bytes = data.get(offset..offset + 4).ok_or_else(|| {
        LiteDroidError::KernelLoadFailed(format!("bzImage header is missing field at {offset:#x}"))
    })?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn write_u16(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
