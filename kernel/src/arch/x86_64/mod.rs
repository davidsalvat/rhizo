//! x86_64 platform code: everything that touches hardware lives here.

pub mod boot;
pub mod console;
pub mod cpu;
#[cfg(any(feature = "ktest", feature = "bench-cell"))]
pub mod irq;
pub mod platform;
#[cfg(any(feature = "ktest", feature = "bench-cell"))]
pub mod pmu;
pub mod port;
pub mod power;
#[cfg(any(feature = "ktest", feature = "bench-cell"))]
pub mod random;
pub mod serial;
#[cfg(any(feature = "ktest", feature = "bench-cell"))]
pub mod smp;
#[cfg(feature = "ktest")]
pub mod tests;
pub mod timer;
#[cfg(any(feature = "ktest", feature = "bench-cell"))]
pub mod user;

/// Stops the CPU forever with interrupts disabled.
pub fn halt_forever() -> ! {
    loop {
        // SAFETY: `cli; hlt` only stops this CPU; nothing else depends on it.
        unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack)) };
    }
}
