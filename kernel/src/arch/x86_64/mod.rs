//! x86_64 platform code: everything that touches hardware lives here.

pub mod boot;
pub mod port;
pub mod qemu;
pub mod serial;
pub mod tsc;

/// Stops the CPU forever with interrupts disabled.
pub fn halt_forever() -> ! {
    loop {
        // SAFETY: `cli; hlt` only stops this CPU; nothing else depends on it.
        unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack)) };
    }
}
