//! Interrupts for preemption (design decisions D2–D5 of `add-scheduler`):
//! the virtual timer (`CNTV_*`, PPI 27) through a GICv3; and the wake-up
//! between CPUs (SGI 0, spec `multi-core`).
//!
//! Interrupts are taken only from cell code (EL0); the kernel runs with them
//! masked. Idling waits with `wfi`, which wakes on a pending interrupt even
//! while it is masked, and acknowledges it without taking an exception.

use super::MMIO_BASE;
use super::platform::gic_regions;
use crate::memory;
use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};
use rhizo_core::smp::MAX_CPUS;
use rhizo_mm::FRAME_SIZE;
use rhizo_mm::paging::PageFlags;

const TIMER_PPI: u32 = 27;
/// Software-generated interrupt that wakes an idle CPU.
const WAKE_SGI: u32 = 0;
const SPURIOUS: u64 = 1023;
/// `GICR_TYPER`: affinity in bits 63:32, `Last` (bit 4), `VLPIS` (bit 1).
const GICR_TYPER: u64 = 0x8;
/// Bytes per redistributor: `RD_base` and `SGI_base` frames, plus two
/// more with virtual LPIs (GICv4).
const REDIST: u64 = 0x2_0000;

/// Physical base of the redistributors.
static GICR_BASE: AtomicU64 = AtomicU64::new(0);
/// Virtual address (MMIO window) of each CPU's redistributor (`RD_base`;
/// `SGI_base` follows 64 KiB later), by CPU number; 0 before its `init`.
static GICR: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// Counter ticks between two timer interrupts (the periodic tick).
static TICK: AtomicU64 = AtomicU64::new(0);

fn write32(addr: u64, value: u32) {
    // SAFETY: `addr` is a GIC register in the MMIO window, mapped as device
    // memory by `platform::map_devices` or `map_redistributors`.
    unsafe { (addr as *mut u32).write_volatile(value) }
}

fn read32(addr: u64) -> u32 {
    // SAFETY: as for `write32`.
    unsafe { (addr as *const u32).read_volatile() }
}

fn read64(addr: u64) -> u64 {
    // SAFETY: as for `write32`.
    unsafe { (addr as *const u64).read_volatile() }
}

/// This CPU's affinity (`MPIDR_EL1` Aff3..Aff0), as the device tree and
/// `GICR_TYPER` write it.
pub fn mpidr() -> u64 {
    let mpidr: u64;
    // SAFETY: reading an ID register has no side effects.
    unsafe { asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack, preserves_flags)) };
    mpidr & 0xff_00ff_ffff
}

/// Distance between two redistributors (the first one's `VLPIS` decides).
fn redist_stride() -> u64 {
    let first = MMIO_BASE + GICR_BASE.load(Ordering::Relaxed);
    if read64(first + GICR_TYPER) & 2 != 0 {
        2 * REDIST
    } else {
        REDIST
    }
}

/// The redistributor (virtual `RD_base`) whose affinity is `mpidr`.
fn redistributor(mpidr: u64) -> Option<u64> {
    let (aff3, low) = (mpidr >> 32, mpidr & 0xff_ffff);
    let affinity = (aff3 << 24) | low;
    let stride = redist_stride();
    let mut rd = MMIO_BASE + GICR_BASE.load(Ordering::Relaxed);
    for _ in 0..MAX_CPUS {
        let typer = read64(rd + GICR_TYPER);
        if typer >> 32 == affinity {
            return Some(rd);
        }
        if typer & (1 << 4) != 0 {
            return None;
        }
        rd += stride;
    }
    None
}

/// Maps the redistributors of `cpus` CPUs (the first one is mapped at
/// boot), so each secondary CPU finds its own.
pub fn map_redistributors(cpus: usize) {
    let Some([_, (base, size)]) = gic_regions() else {
        return;
    };
    let len = (cpus as u64 * redist_stride()).min(size);
    let flags = PageFlags::WRITABLE | PageFlags::NO_EXECUTE | PageFlags::DEVICE;
    for page in (base..base + len).step_by(FRAME_SIZE as usize) {
        if memory::translate(MMIO_BASE + page).is_none() {
            memory::map_page(MMIO_BASE + page, page, flags).expect("map a GIC redistributor");
        }
    }
}

/// Sets up the GIC and starts the timer as a periodic tick every `tick_ns`
/// (design D4: re-armed only when it fires, so switching cells never
/// reprograms it). Without a GICv3 in the device tree, cells are not
/// preempted (there is no interrupt to do it).
pub fn init(tick_ns: u64) {
    let Some([(d, _), (r, _)]) = gic_regions() else {
        return;
    };
    let gicd = MMIO_BASE + d;
    // Distributor: affinity routing and group 1 enabled (bits valid in both
    // the single- and two-security-state views).
    write32(gicd, (1 << 4) | (1 << 1) | 1);
    while read32(gicd) & (1 << 31) != 0 {}
    GICR_BASE.store(r, Ordering::Relaxed);
    init_cpu();
    TICK.store(ns_to_ticks(tick_ns), Ordering::Relaxed);
    arm(ns_to_ticks(tick_ns));
}

/// Sets up this CPU's redistributor and CPU interface: the timer PPI and
/// the wake-up SGI enabled, interrupts left masked in PSTATE. The timer
/// stays off until armed.
pub fn init_cpu() {
    if GICR_BASE.load(Ordering::Relaxed) == 0 {
        return;
    }
    let Some(gicr) = redistributor(mpidr()) else {
        return;
    };
    // Redistributor: wake it up.
    let waker = gicr + 0x14;
    write32(waker, read32(waker) & !(1 << 1));
    while read32(waker) & (1 << 2) != 0 {}
    // The timer PPI and the wake-up SGI: group 1, middle priority, enabled.
    let sgi = gicr + 0x1_0000;
    let bits = (1 << TIMER_PPI) | (1 << WAKE_SGI);
    write32(sgi + 0x80, read32(sgi + 0x80) | bits);
    for intid in [TIMER_PPI, WAKE_SGI] {
        // SAFETY: byte access to the priority register of the interrupt.
        unsafe { ((sgi + 0x400 + intid as u64) as *mut u8).write_volatile(0x80) };
    }
    write32(sgi + 0x100, bits);
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
    GICR[crate::arch::cpu::index()].store(gicr, Ordering::Relaxed);
}

fn ns_to_ticks(ns: u64) -> u64 {
    let ticks = ns as u128 * super::timer::frequency() as u128 / 1_000_000_000;
    (ticks as u64).clamp(1, u32::MAX as u64)
}

/// Routes device interrupt `intid` (an SPI, from the device tree) to the
/// boot CPU, edge-triggered, group 1, middle priority, enabled (design D2
/// of `add-virtio-interrupts`): it wakes the boot CPU from `wfi`, or
/// interrupts the cell code it runs. Returns whether it could.
pub fn route_device(intid: u32) -> bool {
    let Some([(d, _), _]) = gic_regions() else {
        return false;
    };
    if !(32..1020).contains(&intid) || !available() {
        return false;
    }
    let gicd = MMIO_BASE + d;
    let (word, bit) = (intid as u64 / 32 * 4, 1u32 << (intid % 32));
    // Group 1.
    write32(gicd + 0x80 + word, read32(gicd + 0x80 + word) | bit);
    // SAFETY: byte access to the priority register of the interrupt.
    unsafe { ((gicd + 0x400 + intid as u64) as *mut u8).write_volatile(0x80) };
    // Edge-triggered (bit 1 of its two configuration bits).
    let cfg = gicd + 0xc00 + intid as u64 / 16 * 4;
    write32(cfg, read32(cfg) | 2 << (2 * (intid % 16)));
    // To the boot CPU (affinity routing: Aff3 in bits 39:32).
    let aff = mpidr();
    let route = ((aff >> 32) << 32) | (aff & 0xff_ffff);
    // SAFETY: 64-bit access to the GICD_IROUTER of the interrupt, in the
    // distributor mapped as device memory.
    unsafe { ((gicd + 0x6000 + intid as u64 * 8) as *mut u64).write_volatile(route) };
    // Enabled.
    write32(gicd + 0x100 + word, bit);
    true
}

/// Whether preemption is available (a GICv3 was found).
pub fn available() -> bool {
    GICR[0].load(Ordering::Relaxed) != 0
}

/// Arms the timer to fire after `ticks` counter ticks.
fn arm(ticks: u64) {
    // SAFETY: programming this CPU's virtual timer only.
    unsafe {
        asm!(
            "msr cntv_tval_el0, {t}",
            "msr cntv_ctl_el0, {on}",
            "isb",
            t = in(reg) ticks,
            on = in(reg) 1u64,
            options(nostack)
        );
    }
}

/// Starts this CPU's periodic tick (a secondary CPU running cells).
pub fn start_tick() {
    arm(TICK.load(Ordering::Relaxed));
}

/// Stops this CPU's tick (a secondary CPU going idle for good).
pub fn stop_tick() {
    // SAFETY: disabling this CPU's virtual timer only.
    unsafe { asm!("msr cntv_ctl_el0, xzr", "isb", options(nostack)) };
}

/// Makes the periodic tick fire every `ns` from now on (spec `profiling`:
/// sampling by the timer where there is no PMU).
#[cfg(feature = "sampling")]
pub fn set_tick_ns(ns: u64) {
    TICK.store(ns_to_ticks(ns), Ordering::Relaxed);
    start_tick();
}

/// Enables private peripheral interrupt `intid` (16–31) on this CPU, like
/// the timer's; returns whether there is a GIC to do it.
#[cfg(feature = "sampling")]
pub fn enable_ppi(intid: u32) -> bool {
    let gicr = GICR[crate::arch::cpu::index()].load(Ordering::Relaxed);
    if gicr == 0 || !(16..32).contains(&intid) {
        return false;
    }
    let sgi = gicr + 0x1_0000;
    write32(sgi + 0x80, read32(sgi + 0x80) | (1 << intid));
    // SAFETY: byte access to the priority register of the interrupt.
    unsafe { ((sgi + 0x400 + intid as u64) as *mut u8).write_volatile(0x80) };
    write32(sgi + 0x100, 1 << intid);
    true
}

/// Acknowledges a pending interrupt (if any) and returns its INTID; a timer
/// interrupt re-arms the periodic tick.
pub fn acknowledge() -> u64 {
    let intid: u64;
    // SAFETY: reading IAR1 acknowledges the highest-priority pending group 1
    // interrupt.
    unsafe { asm!("mrs {}, S3_0_C12_C12_0", out(reg) intid, options(nostack)) };
    if intid == TIMER_PPI as u64 {
        arm(TICK.load(Ordering::Relaxed));
    }
    if intid != SPURIOUS {
        // SAFETY: ends the interrupt acknowledged above.
        unsafe { asm!("msr S3_0_C12_C12_1, {}", "isb", in(reg) intid, options(nostack)) };
    }
    intid
}

/// Waits about `ns` nanoseconds (or until any interrupt): with interrupts
/// masked, `wfi` still wakes up when one is pending. Acknowledging it
/// restarts the periodic tick.
pub fn wait_ns(ns: u64) {
    arm(ns_to_ticks(ns));
    // SAFETY: waiting for an interrupt has no other effect.
    unsafe { asm!("dsb sy", "wfi", options(nostack)) };
    acknowledge();
}

/// Waits for an interrupt (a wake-up), with the timer left as it is.
pub fn wait() {
    // SAFETY: waiting for an interrupt has no other effect.
    unsafe { asm!("dsb sy", "wfi", options(nostack)) };
    acknowledge();
}

/// Sends the wake-up SGI to the CPU whose affinity is `mpidr`, after the
/// stores made so far are visible to it.
pub fn wake(mpidr: u64) {
    let (aff0, aff1, aff2, aff3) = (
        mpidr & 0xff,
        (mpidr >> 8) & 0xff,
        (mpidr >> 16) & 0xff,
        mpidr >> 32,
    );
    // ICC_SGI1R_EL1: Aff3, range selector (Aff0 / 16), Aff2, INTID, Aff1,
    // target list (bit Aff0 % 16).
    let value = (aff3 << 48)
        | ((aff0 >> 4) << 44)
        | (aff2 << 32)
        | ((WAKE_SGI as u64) << 24)
        | (aff1 << 16)
        | (1 << (aff0 & 0xf));
    // SAFETY: generating an SGI only interrupts the target CPU; the barrier
    // orders the caller's stores before it.
    unsafe { asm!("dsb ishst", "msr S3_0_C12_C11_5, {}", "isb", in(reg) value, options(nostack)) };
}
