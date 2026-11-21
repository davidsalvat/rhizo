//! Cycle counter: the Time Stamp Counter, and its conversion to time.
//!
//! The TSC frequency is found lazily, the first time a conversion is needed
//! (never on the boot path): from KVM's paravirtual clock when running on
//! KVM (QEMU with KVM, Firecracker, Cloud Hypervisor), else from CPUID leaves
//! 0x15/0x16, else by timing the legacy PIT for 10 ms, else (in builds that
//! run cells) against the local APIC timer.

use super::boot::layout;
use super::port::{inb, outb};
use crate::sync::Global;
use core::arch::x86_64::__cpuid;
use core::sync::atomic::{AtomicU64, Ordering};
use rhizo_hal::CycleCounter;

/// The CPU's time stamp counter.
#[derive(Debug, Clone, Copy)]
pub struct Counter;

impl CycleCounter for Counter {
    fn cycles(&self) -> u64 {
        // SAFETY: `rdtsc` is available on every x86_64 CPU and only reads.
        unsafe { core::arch::x86_64::_rdtsc() }
    }
}

/// Conversion `ns = (cycles << shift) * mul >> 32` (`shift` may be negative),
/// packed as `mul << 8 | (shift as u8)`; 0 = not found yet, 1 = unknown.
static CONVERSION: AtomicU64 = AtomicU64::new(0);

/// Converts TSC cycles to nanoseconds, or `None` if the frequency is unknown.
pub fn cycles_to_ns(cycles: u64) -> Option<u64> {
    let mut packed = CONVERSION.load(Ordering::Relaxed);
    if packed == 0 {
        packed = calibrate().map_or(1, |(mul, shift)| ((mul as u64) << 8) | shift as u8 as u64);
        CONVERSION.store(packed, Ordering::Relaxed);
    }
    if packed == 1 {
        return None;
    }
    let (mul, shift) = ((packed >> 8) as u128, packed as u8 as i8);
    let scaled = if shift >= 0 {
        (cycles as u128) << shift
    } else {
        (cycles as u128) >> -shift
    };
    Some(((scaled * mul) >> 32) as u64)
}

fn calibrate() -> Option<(u32, i8)> {
    kvm_clock().or_else(|| {
        cpuid_frequency()
            .or_else(pit_frequency)
            .or_else(apic_frequency)
            .map(from_hz)
    })
}

/// In builds that run cells, the local APIC timer is set up and can serve as
/// a reference.
fn apic_frequency() -> Option<u64> {
    #[cfg(feature = "cells")]
    return super::irq::tsc_hz();
    #[cfg(not(feature = "cells"))]
    None
}

/// `(mul, shift)` for a frequency in Hz: the smallest shift that makes
/// `mul = 10^9 * 2^32 / (hz << shift)` fit in 32 bits.
fn from_hz(hz: u64) -> (u32, i8) {
    let mut shift = 0;
    loop {
        let mul = (1_000_000_000u128 << 32) / ((hz as u128) << shift);
        if mul <= u32::MAX as u128 || shift == 31 {
            return (mul.min(u32::MAX as u128) as u32, shift);
        }
        shift += 1;
    }
}

/// `pvclock_vcpu_time_info`, written by KVM.
#[repr(C, align(32))]
struct PvClock {
    version: u32,
    pad0: u32,
    tsc_timestamp: u64,
    system_time: u64,
    tsc_to_system_mul: u32,
    tsc_shift: i8,
    flags: u8,
    pad: [u8; 2],
}

static PV_CLOCK: Global<PvClock> = Global::new(PvClock {
    version: 0,
    pad0: 0,
    tsc_timestamp: 0,
    system_time: 0,
    tsc_to_system_mul: 0,
    tsc_shift: 0,
    flags: 0,
    pad: [0; 2],
});

/// KVM's own TSC-to-nanoseconds factors (`MSR_KVM_SYSTEM_TIME_NEW`).
fn kvm_clock() -> Option<(u32, i8)> {
    const MSR_KVM_SYSTEM_TIME_NEW: u32 = 0x4b56_4d01;
    let sig = __cpuid(0x4000_0000);
    let kvm = [sig.ebx, sig.ecx, sig.edx] == [0x4b4d_564b, 0x564b_4d56, 0x4d];
    if !kvm || __cpuid(0x4000_0001).eax & (1 << 3) == 0 {
        return None; // not KVM, or no KVM_FEATURE_CLOCKSOURCE2
    }
    PV_CLOCK.with(|clock| {
        let phys = layout::virt_to_phys(clock as *mut PvClock as u64);
        // SAFETY: the MSR exists (feature bit above); KVM writes the 32-byte
        // structure at `phys`, a static the kernel owns, and stops when the
        // MSR is cleared again below.
        unsafe { super::cpu::wrmsr(MSR_KVM_SYSTEM_TIME_NEW, phys | 1) };
        // SAFETY: KVM filled the structure during the WRMSR exit; volatile
        // because the hypervisor, not this code, wrote it.
        let (mul, shift) = unsafe {
            (
                core::ptr::addr_of!(clock.tsc_to_system_mul).read_volatile(),
                core::ptr::addr_of!(clock.tsc_shift).read_volatile(),
            )
        };
        // SAFETY: disables the updates enabled above.
        unsafe { super::cpu::wrmsr(MSR_KVM_SYSTEM_TIME_NEW, 0) };
        (mul != 0).then_some((mul, shift))
    })
}

/// TSC frequency from CPUID leaf 0x15 (crystal ratio) or 0x16 (base MHz).
fn cpuid_frequency() -> Option<u64> {
    let max = __cpuid(0).eax;
    if max >= 0x15 {
        let l = __cpuid(0x15);
        if l.eax != 0 && l.ebx != 0 && l.ecx != 0 {
            return Some(l.ecx as u64 * l.ebx as u64 / l.eax as u64);
        }
    }
    if max >= 0x16 {
        let mhz = __cpuid(0x16).eax & 0xffff;
        if mhz != 0 {
            return Some(mhz as u64 * 1_000_000);
        }
    }
    None
}

/// Times 10 ms of the PIT (channel 2, gated through port 0x61).
fn pit_frequency() -> Option<u64> {
    const PIT_HZ: u64 = 1_193_182;
    const COUNT: u16 = 11_932; // ~10 ms
    // SAFETY: ports 0x42, 0x43 and 0x61 belong to the legacy PIT and speaker
    // gate; nothing else in the kernel uses them.
    unsafe {
        let gate = inb(0x61);
        if gate == 0xff {
            return None; // no PIT
        }
        outb(0x61, (gate & !0x02) | 0x01); // gate on, speaker off
        outb(0x43, 0xb0); // channel 2, low/high byte, mode 0
        outb(0x42, COUNT as u8);
        outb(0x42, (COUNT >> 8) as u8);
        let start = Counter.cycles();
        let mut spins = 0u64;
        while inb(0x61) & 0x20 == 0 {
            spins += 1;
            if spins > 100_000_000 {
                return None;
            }
        }
        let cycles = Counter.cycles() - start;
        outb(0x61, gate);
        let hz = cycles * PIT_HZ / COUNT as u64;
        (hz > 250_000_000).then_some(hz)
    }
}
