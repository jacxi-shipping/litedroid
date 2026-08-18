/// Default number of virtual CPUs assigned to a new device.
pub const DEFAULT_VCPU_COUNT: u32 = 4;

/// Default RAM size in megabytes.
pub const DEFAULT_RAM_MB: u64 = 2048;

/// Minimum allowed RAM size in megabytes.
pub const MIN_RAM_MB: u64 = 512;

/// Maximum allowed RAM size in megabytes.
pub const MAX_RAM_MB: u64 = 8192;

/// Guest physical address where the kernel binary is loaded.
pub const KERNEL_LOAD_ADDR: u64 = 0x100_000;

/// Guest physical address where the initramfs is loaded.
pub const INITRAMFS_LOAD_ADDR: u64 = 0x80_000_00;

/// Guest physical address for the device-tree blob.
pub const DEVICE_TREE_ADDR: u64 = 0x4000_0000;

/// Base address for virtio MMIO devices.
pub const VIRTIO_MMIO_START: u64 = 0xfea0_0000;

/// Size of each virtio MMIO region.
pub const VIRTIO_MMIO_SIZE: u64 = 0x200;

/// Maximum number of virtio devices the VMM will support.
pub const MAX_VIRTIO_DEVICES: usize = 16;

/// I/O base port for the emulated 16550 serial UART.
pub const SERIAL_IO_BASE: u16 = 0x3f8;

/// Unix domain socket path used by the background daemon.
pub const DAEMON_SOCKET_PATH: &str = "/tmp/litedroid-daemon.sock";

/// PID file written by the background daemon.
pub const DAEMON_PID_PATH: &str = "/tmp/litedroid-daemon.pid";

/// Default directory for runtime data (images, snapshots, …).
pub const DEFAULT_DATA_DIR: &str = "~/.local/share/litedroid";

/// Default directory for configuration files.
pub const DEFAULT_CONFIG_DIR: &str = "~/.config/litedroid";

/// TCP port the in-guest adbd listens on.
pub const ADB_PORT: u16 = 5555;

/// Prefix used when naming the TAP network interface.
pub const TAP_IFACE_PREFIX: &str = "litedroid-tap";

/// Wire protocol version spoken over the IPC channel.
pub const IPC_PROTOCOL_VERSION: u32 = 1;

/// Default kernel command-line passed to the guest Linux kernel.
pub const DEFAULT_KERNEL_CMDLINE: &str = "console=ttyS0 \
    androidboot.hardware=litedroid \
    androidboot.serialno=LITEDROID001 \
    androidboot.mode=normal \
    root=/dev/ram0 rw init=/init";
