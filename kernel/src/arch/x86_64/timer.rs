//! Cycle counter: the Time Stamp Counter.

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

/// TSC frequency is not calibrated yet, so cycles cannot be converted.
pub fn cycles_to_ns(_cycles: u64) -> Option<u64> {
    None
}
