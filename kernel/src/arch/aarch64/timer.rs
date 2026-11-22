//! Cycle counter: the generic timer's virtual count (`CNTVCT_EL0`).

use rhizo_hal::CycleCounter;

/// The generic timer's virtual counter.
#[derive(Debug, Clone, Copy)]
pub struct Counter;

impl CycleCounter for Counter {
    fn cycles(&self) -> u64 {
        let value: u64;
        // SAFETY: reading the virtual counter has no side effects; the `isb`
        // keeps it from being read early.
        unsafe {
            core::arch::asm!("isb", "mrs {}, cntvct_el0", out(reg) value, options(nomem, nostack))
        };
        value
    }
}

/// Counter frequency in Hz (`CNTFRQ_EL0`).
pub fn frequency() -> u64 {
    let value: u64;
    // SAFETY: reading the counter frequency has no side effects.
    unsafe { core::arch::asm!("mrs {}, cntfrq_el0", out(reg) value, options(nomem, nostack)) };
    value
}

/// Converts counter ticks to nanoseconds.
pub fn cycles_to_ns(cycles: u64) -> Option<u64> {
    const NS: u64 = 1_000_000_000;
    let hz = frequency();
    if hz == 0 {
        return None;
    }
    // A whole number of nanoseconds per tick (QEMU's 62.5 MHz, 1 GHz
    // counters): one multiplication, the same result as the 128-bit
    // division, which a clock read on every switch would otherwise pay.
    if NS.is_multiple_of(hz) {
        return Some((u128::from(cycles) * u128::from(NS / hz)) as u64);
    }
    Some((u128::from(cycles) * u128::from(NS) / u128::from(hz)) as u64)
}
