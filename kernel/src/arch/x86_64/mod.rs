//! x86_64 platform code: everything that touches hardware lives here.

pub mod boot;
pub mod console;
pub mod cpu;
#[cfg(feature = "cells")]
pub mod irq;
pub mod platform;
pub mod port;
pub mod power;
pub mod serial;
#[cfg(feature = "cells")]
pub mod smp;
#[cfg(feature = "ktest")]
pub mod tests;
pub mod timer;
#[cfg(feature = "cells")]
pub mod user;

/// Stops the CPU forever with interrupts disabled.
pub fn halt_forever() -> ! {
    loop {
        // SAFETY: `cli; hlt` only stops this CPU; nothing else depends on it.
        unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack)) };
    }
}
