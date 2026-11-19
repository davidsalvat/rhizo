//! Time Stamp Counter.

use rhizo_hal::CycleCounter;

/// The CPU's time stamp counter.
#[derive(Debug, Clone, Copy)]
pub struct Tsc;

impl CycleCounter for Tsc {
    fn cycles(&self) -> u64 {
        // SAFETY: `rdtsc` is available on every x86_64 CPU and only reads.
        unsafe { core::arch::x86_64::_rdtsc() }
    }
}
