//! Interrupts for preemption (design decisions D2–D5 of `add-scheduler`):
//! the virtual timer (`CNTV_*`, PPI 27) through a GICv3.
//!
//! Interrupts are taken only from cell code (EL0); the kernel runs with them
//! masked. Idling waits with `wfi`, which wakes on a pending interrupt even
//! while it is masked, and acknowledges it without taking an exception.

use super::MMIO_BASE;
use super::platform::gic_regions;
use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

const TIMER_PPI: u32 = 27;
const SPURIOUS: u64 = 1023;

/// Virtual addresses (MMIO window) of the distributor and of CPU 0's
/// redistributor (`RD_base`; `SGI_base` follows 64 KiB later).
static GICD: AtomicU64 = AtomicU64::new(0);
static GICR: AtomicU64 = AtomicU64::new(0);

fn write32(addr: u64, value: u32) {
    // SAFETY: `addr` is a GIC register in the MMIO window, mapped as device
    // memory by `platform::map_devices`.
    unsafe { (addr as *mut u32).write_volatile(value) }
}

fn read32(addr: u64) -> u32 {
    // SAFETY: as for `write32`.
    unsafe { (addr as *const u32).read_volatile() }
}

/// Sets up the GIC for the timer interrupt. Without a GICv3 in the device
/// tree, cells are not preempted (there is no interrupt to do it).
pub fn init() {
    let Some([(d, _), (r, _)]) = gic_regions() else {
        return;
    };
    let (gicd, gicr) = (MMIO_BASE + d, MMIO_BASE + r);
    // Distributor: affinity routing and group 1 enabled (bits valid in both
    // the single- and two-security-state views).
    write32(gicd, (1 << 4) | (1 << 1) | 1);
    while read32(gicd) & (1 << 31) != 0 {}
    // Redistributor: wake it up.
    let waker = gicr + 0x14;
    write32(waker, read32(waker) & !(1 << 1));
    while read32(waker) & (1 << 2) != 0 {}
    // The timer PPI: group 1, middle priority, enabled.
    let sgi = gicr + 0x1_0000;
    write32(sgi + 0x80, read32(sgi + 0x80) | (1 << TIMER_PPI));
    // SAFETY: byte access to the priority register of the PPI.
    unsafe { ((sgi + 0x400 + TIMER_PPI as u64) as *mut u8).write_volatile(0x80) };
    write32(sgi + 0x100, 1 << TIMER_PPI);
    // CPU interface through system registers: priority mask open, group 1 on.
    // SAFETY: enabling the system-register interface and group 1 interrupts
    // of this CPU; interrupts stay masked in PSTATE while the kernel runs.
    unsafe {
        asm!(
            "mrs {t}, S3_0_C12_C12_5",
            "orr {t}, {t}, #1",
            "msr S3_0_C12_C12_5, {t}",
            "isb",
            "mov {t}, #0xff",
            "msr S3_0_C4_C6_0, {t}",
            "mov {t}, #1",
            "msr S3_0_C12_C12_7, {t}",
            "isb",
            t = out(reg) _,
            options(nostack)
        );
    }
    GICD.store(gicd, Ordering::Relaxed);
    GICR.store(gicr, Ordering::Relaxed);
}

/// Whether preemption is available (a GICv3 was found).
pub fn available() -> bool {
    GICR.load(Ordering::Relaxed) != 0
}

/// Arms the timer to fire after `ns` nanoseconds.
pub fn arm_after_ns(ns: u64) {
    let ticks = (ns as u128 * super::timer::frequency() as u128 / 1_000_000_000) as u64;
    // SAFETY: programming this CPU's virtual timer only.
    unsafe {
        asm!(
            "msr cntv_tval_el0, {t}",
            "msr cntv_ctl_el0, {on}",
            "isb",
            t = in(reg) ticks.clamp(1, u32::MAX as u64),
            on = in(reg) 1u64,
            options(nostack)
        );
    }
}

/// Acknowledges a pending interrupt (if any) and silences the timer until it
/// is armed again.
pub fn acknowledge() {
    let intid: u64;
    // SAFETY: reading IAR1 acknowledges the highest-priority pending group 1
    // interrupt; masking the timer stops it from asserting again.
    unsafe {
        asm!("mrs {}, S3_0_C12_C12_0", out(reg) intid, options(nostack));
        asm!("msr cntv_ctl_el0, {}", "isb", in(reg) 0b11u64, options(nostack));
    }
    if intid != SPURIOUS {
        // SAFETY: ends the interrupt acknowledged above.
        unsafe { asm!("msr S3_0_C12_C12_1, {}", "isb", in(reg) intid, options(nostack)) };
    }
}

/// Waits until an interrupt is pending (with interrupts masked, `wfi` still
/// wakes up), then acknowledges it.
pub fn wait() {
    // SAFETY: waiting for an interrupt has no other effect.
    unsafe { asm!("dsb sy", "wfi", options(nostack)) };
    acknowledge();
}
