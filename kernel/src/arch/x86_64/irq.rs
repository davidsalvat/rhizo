//! Interrupts for preemption (design decisions D2–D5 of `add-scheduler`):
//! the local APIC timer in one-shot mode, vector 32.
//!
//! Interrupts are enabled only while cell code runs (ring 3, `RFLAGS.IF`)
//! and, briefly, in [`wait`]. The local APIC timer of KVM and QEMU counts at
//! 1 GHz; with a divider of 16 one count is 16 ns. On other hardware the
//! slice would differ in length, but a running cell is still preempted.

use super::cpu::{rdmsr, wrmsr};
use super::port::outb;
use crate::memory::{self, DirectMap};
use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};
use rhizo_mm::paging::PageFlags;

/// Vector of the timer interrupt.
pub const TIMER_VECTOR: u8 = 32;
/// Vector of the wake-up between CPUs (spec `multi-core`).
pub const WAKE_VECTOR: u8 = 33;
/// Vector of the performance counters' sampling interrupt (spec
/// `profiling`).
pub const SAMPLE_VECTOR: u8 = 34;
/// Vector of spurious APIC interrupts (no EOI).
pub const SPURIOUS_VECTOR: u8 = 255;

const IA32_APIC_BASE: u32 = 0x1b;
const ID: u32 = 0x20;
const SVR: u32 = 0xf0;
const ICR_LOW: u32 = 0x300;
const ICR_HIGH: u32 = 0x310;
/// ICR: level assert, and the INIT and STARTUP delivery modes.
const ASSERT: u32 = 1 << 14;
const INIT: u32 = 0b101 << 8;
const STARTUP: u32 = 0b110 << 8;
const EOI: u32 = 0xb0;
const LVT_TIMER: u32 = 0x320;
const LVT_PERF: u32 = 0x340;
const INITIAL_COUNT: u32 = 0x380;
const CURRENT_COUNT: u32 = 0x390;
const DIVIDE: u32 = 0x3e0;
const NS_PER_COUNT: u64 = 16;
const PERIODIC: u32 = 1 << 17;

/// Initial count of the periodic tick (0 before `init`).
static TICK: AtomicU64 = AtomicU64::new(0);

/// 1 when using x2APIC MSRs, else the virtual address of the xAPIC page
/// (0 before `init`).
static APIC: AtomicU64 = AtomicU64::new(0);

fn write(reg: u32, value: u32) {
    match APIC.load(Ordering::Relaxed) {
        0 => {}
        // SAFETY: x2APIC registers are MSRs 0x800 + offset / 16.
        1 => unsafe { wrmsr(0x800 + (reg >> 4), value as u64) },
        // SAFETY: the xAPIC page is mapped as device memory by `init`.
        base => unsafe { ((base + reg as u64) as *mut u32).write_volatile(value) },
    }
}

fn read(reg: u32) -> u32 {
    match APIC.load(Ordering::Relaxed) {
        0 => 0,
        // SAFETY: as in `write`.
        1 => unsafe { rdmsr(0x800 + (reg >> 4)) as u32 },
        // SAFETY: as in `write`.
        base => unsafe { ((base + reg as u64) as *const u32).read_volatile() },
    }
}

/// TSC frequency measured against 10 ms of the APIC timer (1 GHz on KVM and
/// QEMU): the last resort of the TSC calibration, for QEMU without KVM or
/// a PIT.
pub fn tsc_hz() -> Option<u64> {
    if !available() {
        return None;
    }
    use rhizo_hal::CycleCounter;
    let tsc = super::timer::Counter;
    write(DIVIDE, 0b1011); // divide by 1
    write(LVT_TIMER, (1 << 16) | TIMER_VECTOR as u32); // masked
    write(INITIAL_COUNT, u32::MAX);
    let (t0, c0) = (tsc.cycles(), read(CURRENT_COUNT));
    let mut c1 = c0;
    while c0 - c1 < 10_000_000 && c1 != 0 {
        c1 = read(CURRENT_COUNT);
    }
    let t1 = tsc.cycles();
    write(DIVIDE, 0b0011);
    tick();
    let counts = (c0 - c1) as u64;
    (counts > 0).then(|| ((t1 - t0) as u128 * 1_000_000_000 / counts as u128) as u64)
}

/// (Re)starts the periodic tick.
fn tick() {
    let count = TICK.load(Ordering::Relaxed) as u32;
    write(LVT_TIMER, PERIODIC | TIMER_VECTOR as u32);
    write(INITIAL_COUNT, count);
}

/// Enables this CPU's local APIC, in x2APIC mode when `x2apic`.
fn enable(x2apic: bool) {
    // SAFETY: IA32_APIC_BASE exists on every x86_64 CPU; setting EN (and
    // EXTD for x2APIC) enables the local APIC.
    unsafe {
        let base = rdmsr(IA32_APIC_BASE);
        let extd = if x2apic { 1 << 10 } else { 0 };
        wrmsr(IA32_APIC_BASE, base | (1 << 11) | extd);
    }
}

/// Masks the legacy PIC, enables the local APIC and starts its timer as a
/// periodic tick every `tick_ns` on [`TIMER_VECTOR`] (design D4: armed once,
/// so switching cells never reprograms it; under virtualization every
/// reprogramming is a VM exit).
pub fn init(tick_ns: u64) {
    // SAFETY: masking every line of both legacy PICs (writes to absent
    // ports, as on QEMU microvm without a PIC, are ignored).
    unsafe {
        outb(0x21, 0xff);
        outb(0xa1, 0xff);
    }
    let x2apic = core::arch::x86_64::__cpuid(1).ecx & (1 << 21) != 0;
    // SAFETY: IA32_APIC_BASE exists on every x86_64 CPU; setting EN (and
    // EXTD for x2APIC) enables the local APIC.
    let base = unsafe { rdmsr(IA32_APIC_BASE) };
    if x2apic {
        enable(true);
        APIC.store(1, Ordering::Relaxed);
    } else {
        let phys = base & 0xf_ffff_f000;
        let virt = DirectMap::virt(phys);
        if memory::translate(virt).is_none() {
            let flags = PageFlags::WRITABLE | PageFlags::NO_EXECUTE | PageFlags::DEVICE;
            memory::map_page(virt, phys, flags).expect("map the local APIC page");
        }
        enable(false);
        APIC.store(virt, Ordering::Relaxed);
    }
    write(SVR, 0x100 | SPURIOUS_VECTOR as u32);
    write(DIVIDE, 0b0011);
    TICK.store(
        (tick_ns / NS_PER_COUNT).clamp(1, u32::MAX as u64),
        Ordering::Relaxed,
    );
    tick();
}

/// Enables a secondary CPU's local APIC like the boot CPU's, its timer
/// masked (an idle CPU takes no ticks) and counting at the boot CPU's rate
/// (the divider resets to 2, not [`init`]'s 16).
pub fn init_cpu() {
    enable(APIC.load(Ordering::Relaxed) == 1);
    write(SVR, 0x100 | SPURIOUS_VECTOR as u32);
    write(LVT_TIMER, (1 << 16) | TIMER_VECTOR as u32);
    write(DIVIDE, 0b0011);
}

/// This CPU's local APIC identifier (the boot CPU's before `init`).
pub fn apic_id() -> u32 {
    match APIC.load(Ordering::Relaxed) {
        0 => core::arch::x86_64::__cpuid(1).ebx >> 24,
        1 => read(ID),
        _ => read(ID) >> 24,
    }
}

/// Sends an interrupt command to the CPU with APIC identifier `dest`.
fn command(dest: u32, low: u32) {
    match APIC.load(Ordering::Relaxed) {
        0 => {}
        // SAFETY: the x2APIC ICR is MSR 0x830; WRMSR to it is not
        // serializing, so the fence first makes earlier stores visible.
        1 => unsafe {
            asm!("mfence", options(nostack, preserves_flags));
            wrmsr(0x830, ((dest as u64) << 32) | low as u64);
        },
        _ => {
            write(ICR_HIGH, dest << 24);
            write(ICR_LOW, low);
            while read(ICR_LOW) & (1 << 12) != 0 {
                core::hint::spin_loop();
            }
        }
    }
}

/// Sends INIT to the CPU `dest` (it waits for a STARTUP IPI).
pub fn send_init(dest: u32) {
    command(dest, INIT | ASSERT);
}

/// Sends STARTUP to the CPU `dest`: it starts in real mode at page `page`.
pub fn send_startup(dest: u32, page: u8) {
    command(dest, STARTUP | ASSERT | page as u32);
}

/// Sends the wake-up interrupt to the CPU `dest`.
pub fn wake(dest: u32) {
    command(dest, ASSERT | WAKE_VECTOR as u32);
}

/// Waits for an interrupt (a wake-up), with interrupts enabled for the
/// length of `hlt` only.
pub fn wait() {
    // SAFETY: as in `wait_ns`.
    unsafe { asm!("sti", "hlt", "cli", options(nomem, nostack)) };
    acknowledge();
}

/// Whether preemption is available (always, once `init` ran).
pub fn available() -> bool {
    APIC.load(Ordering::Relaxed) != 0
}

/// Makes the periodic tick fire every `ns` from now on (spec `profiling`:
/// sampling by the timer where there is no PMU).
#[cfg(feature = "sampling")]
pub fn set_tick_ns(ns: u64) {
    TICK.store(
        (ns / NS_PER_COUNT).clamp(1, u32::MAX as u64),
        Ordering::Relaxed,
    );
    tick();
}

/// Delivers the performance counters' overflow on [`SAMPLE_VECTOR`]
/// (`on`), or masks it.
pub(super) fn sample_interrupt(on: bool) {
    let mask = if on { 0 } else { 1 << 16 };
    write(LVT_PERF, mask | SAMPLE_VECTOR as u32);
}

/// Starts this CPU's periodic tick (a secondary CPU running cells).
pub fn start_tick() {
    tick();
}

/// Stops this CPU's tick (a secondary CPU going idle for good).
pub fn stop_tick() {
    write(LVT_TIMER, (1 << 16) | TIMER_VECTOR as u32);
}

/// Ends the interrupt being handled (the periodic tick re-arms itself).
pub fn acknowledge() {
    write(EOI, 0);
}

/// Waits about `ns` nanoseconds (or until any interrupt), with interrupts
/// enabled for the length of `hlt` only, then goes back to the periodic
/// tick. The IDT stub returns at once for an interrupt taken in ring 0.
pub fn wait_ns(ns: u64) {
    write(LVT_TIMER, TIMER_VECTOR as u32); // one-shot for the wait
    write(
        INITIAL_COUNT,
        (ns / NS_PER_COUNT).clamp(1, u32::MAX as u64) as u32,
    );
    // SAFETY: `sti; hlt` enables interrupts only for the halt (the
    // instruction after `sti` runs before any interrupt), and the stub for
    // vectors 32 and 255 in ring 0 just returns.
    unsafe { asm!("sti", "hlt", "cli", options(nomem, nostack)) };
    acknowledge();
    tick();
}
