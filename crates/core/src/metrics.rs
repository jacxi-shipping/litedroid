use serde::{Deserialize, Serialize};

/// Top-level container for all resource-metric snapshots.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceMetrics {
    pub cpu: CpuStats,
    pub memory: MemoryStats,
    #[allow(dead_code)]
    pub gpu: GpuStats,
    pub network: NetworkStats,
    /// Timestamp (seconds since Unix epoch) when the snapshot was taken.
    pub timestamp: f64,
}

/// CPU utilisation counters.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CpuStats {
    /// Percentage (0-100) of host CPU consumed by the VMM.
    pub usage_percent: f32,
    /// Total guest CPU time in nanoseconds across all vCPUs.
    pub guest_time_ns: u64,
    /// Total host CPU time in nanoseconds consumed by the VMM process.
    pub host_time_ns: u64,
}

/// Guest physical memory utilisation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryStats {
    /// Total guest RAM in bytes.
    pub total_bytes: u64,
    /// Bytes currently resident / allocated by the guest.
    pub used_bytes: u64,
    /// Peak RSS of the VMM process in bytes.
    pub rss_bytes: u64,
}

/// GPU / display rendering statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GpuStats {
    /// Frames rendered in the last measurement window.
    pub fps: u32,
    /// Average frame render time in milliseconds.
    pub frame_time_ms: f32,
}

/// Cumulative network I/O counters.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkStats {
    /// Total bytes received from the guest.
    pub rx_bytes: u64,
    /// Total bytes sent to the guest.
    pub tx_bytes: u64,
    /// Total bytes received on the host-side interface.
    pub host_rx_bytes: u64,
    /// Total bytes transmitted on the host-side interface.
    pub host_tx_bytes: u64,
    /// Number of packets dropped.
    pub dropped_packets: u64,
}
