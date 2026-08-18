//! CPUID filtering utilities tailored for Android guest workloads.

use kvm_bindings::CpuId;
use tracing::debug;

/// KVM CPUID leaf 0x40000000 — hypervisor vendor signature.
const KVM_CPUID_SIGNATURE: &[u8; 12] = b"KVMKVMKVM\0\0\0";

/// Modify `cpuid` in-place so that the exposed feature set is appropriate for
/// running an Android guest on KVM.
///
/// Changes applied:
/// * Enables the **hypervisor-present** bit (CPUID leaf 0x1, ECX bit 31).
/// * Enables **x2APIC** (CPUID leaf 0x1, ECX bit 21).
/// * Sets the KVM hypervisor signature at leaf 0x4000_0000.
/// * Advertises KVM frequency / features at leaves 0x4000_0001+.
pub fn filter_for_android(cpuid: &mut CpuId) {
    debug!("Filtering CPUID for Android guest");

    for entry in cpuid.as_mut_slice().iter_mut() {
        match entry.function {
            // ---- Leaf 0x1: feature flags ----
            0x1 => {
                // ECX: enable hypervisor bit (31) and x2apic (21)
                entry.ecx |= 1u32 << 31; // hypervisor
                entry.ecx |= 1u32 << 21; // x2apic

                // EDX: ensure APIC is present (bit 9)
                entry.edx |= 1u32 << 9; // apic
            }

            // ---- Leaf 0x4000_0000: KVM hypervisor signature ----
            0x4000_0000 => {
                entry.eax = 0x4000_0001; // max supported hypervisor leaf
                entry.ebx = u32::from_le_bytes(KVM_CPUID_SIGNATURE[0..4].try_into().unwrap());
                entry.ecx = u32::from_le_bytes(KVM_CPUID_SIGNATURE[4..8].try_into().unwrap());
                entry.edx = u32::from_le_bytes(KVM_CPUID_SIGNATURE[8..12].try_into().unwrap());
            }

            // ---- Leaf 0x4000_0001: KVM hypervisor features / frequency ----
            0x4000_0001 => {
                // TSC kHz is read from KVM_GET_TSC_KHZ at runtime, but we can
                // expose the "stable TSC" feature here.
                entry.eax = 0; // tsc_khz — filled by kernel on demand
                entry.ebx = 0; // padding
                entry.ecx = 0; // features (none required for Android)
                entry.edx = 0; // features (none required for Android)
            }

            _ => {
                // Leave all other leaves unchanged.
            }
        }
    }
}

/// Determine whether the host CPU supports the **AVX** extension set by
/// inspecting CPUID leaf 1 / ECX bits 27-28.
#[allow(dead_code)]
pub fn supports_avx(cpuid: &CpuId) -> bool {
    for entry in cpuid.as_slice() {
        if entry.function == 0x1 {
            let avx = (entry.ecx >> 27) & 1 == 1;
            let osxsave = (entry.ecx >> 27) & 0b11 == 0b11;
            return avx && osxsave;
        }
    }
    false
}

/// Determine whether the host CPU supports **SSE4.2**.
#[allow(dead_code)]
pub fn supports_sse42(cpuid: &CpuId) -> bool {
    for entry in cpuid.as_slice() {
        if entry.function == 0x1 {
            return (entry.ecx >> 20) & 1 == 1;
        }
    }
    false
}
