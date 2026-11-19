//! AArch64 platform code (EL1, 4 KiB granule, 48-bit virtual addresses).

pub mod boot;
pub mod console;
pub mod cpu;
#[cfg(any(feature = "ktest", feature = "bench-cell"))]
pub mod irq;
pub mod platform;
pub mod power;
#[cfg(feature = "ktest")]
pub mod tests;
pub mod timer;
#[cfg(any(feature = "ktest", feature = "bench-cell"))]
pub mod user;

/// Virtual window where physical 0–4 GiB is mapped as device memory.
pub const MMIO_BASE: u64 = 0xffff_c000_0000_0000;

/// Stops the CPU forever with interrupts masked.
pub fn halt_forever() -> ! {
    loop {
        // SAFETY: masking interrupts and waiting only stops this CPU.
        unsafe { core::arch::asm!("msr daifset, #0xf", "wfi", options(nomem, nostack)) };
    }
}
