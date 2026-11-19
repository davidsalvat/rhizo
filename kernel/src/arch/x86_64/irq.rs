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
/// Vector of spurious APIC interrupts (no EOI).
pub const SPURIOUS_VECTOR: u8 = 255;

const IA32_APIC_BASE: u32 = 0x1b;
const SVR: u32 = 0xf0;
const EOI: u32 = 0xb0;
const LVT_TIMER: u32 = 0x320;
const INITIAL_COUNT: u32 = 0x380;
const CURRENT_COUNT: u32 = 0x390;
const DIVIDE: u32 = 0x3e0;
const NS_PER_COUNT: u64 = 16;

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
    write(INITIAL_COUNT, 0);
    write(DIVIDE, 0b0011);
    write(LVT_TIMER, TIMER_VECTOR as u32);
    let counts = (c0 - c1) as u64;
    (counts > 0).then(|| ((t1 - t0) as u128 * 1_000_000_000 / counts as u128) as u64)
}

/// Masks the legacy PIC and enables the local APIC with its timer on
/// [`TIMER_VECTOR`], one-shot, divided by 16.
pub fn init() {
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
        // SAFETY: as above.
        unsafe { wrmsr(IA32_APIC_BASE, base | (1 << 11) | (1 << 10)) };
        APIC.store(1, Ordering::Relaxed);
    } else {
        let phys = base & 0xf_ffff_f000;
        let virt = DirectMap::virt(phys);
        if memory::translate(virt).is_none() {
            let flags = PageFlags::WRITABLE | PageFlags::NO_EXECUTE | PageFlags::DEVICE;
            memory::map_page(virt, phys, flags).expect("map the local APIC page");
        }
        // SAFETY: as above.
        unsafe { wrmsr(IA32_APIC_BASE, base | (1 << 11)) };
        APIC.store(virt, Ordering::Relaxed);
    }
    write(SVR, 0x100 | SPURIOUS_VECTOR as u32);
    write(DIVIDE, 0b0011);
    write(LVT_TIMER, TIMER_VECTOR as u32);
}

/// Whether preemption is available (always, once `init` ran).
pub fn available() -> bool {
    APIC.load(Ordering::Relaxed) != 0
}

/// Arms the timer to fire after about `ns` nanoseconds.
pub fn arm_after_ns(ns: u64) {
    write(
        INITIAL_COUNT,
        (ns / NS_PER_COUNT).clamp(1, u32::MAX as u64) as u32,
    );
}

/// Ends the interrupt being handled and stops the timer until it is armed
/// again.
pub fn acknowledge() {
    write(INITIAL_COUNT, 0);
    write(EOI, 0);
}

/// Waits for an interrupt with interrupts enabled for the length of `hlt`
/// only, then acknowledges it. The IDT stub returns at once for an
/// interrupt taken in ring 0.
pub fn wait() {
    // SAFETY: `sti; hlt` enables interrupts only for the halt (the
    // instruction after `sti` runs before any interrupt), and the stub for
    // vectors 32 and 255 in ring 0 just returns.
    unsafe { asm!("sti", "hlt", "cli", options(nomem, nostack)) };
    acknowledge();
}
