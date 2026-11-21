//! Reads the CPU's counter directly, without a system call, as libraries do
//! for cheap timestamps (abseil's cycle clock in Node.js): the virtual
//! counter `CNTVCT_EL0` and its frequency `CNTFRQ_EL0` on AArch64, the
//! time-stamp counter on x86_64 (spec `isolation`, what cells may read).
//! Prints `ticks ok` once two readings are nonzero and do not go back.
#![no_std]
#![no_main]

use rhizo_cells::{check, print};

#[cfg(target_arch = "aarch64")]
fn read() -> (u64, u64) {
    let (count, frequency): (u64, u64);
    // SAFETY: reading the counter and its frequency changes nothing.
    unsafe {
        core::arch::asm!(
            "isb",
            "mrs {c}, cntvct_el0",
            "mrs {f}, cntfrq_el0",
            c = out(reg) count,
            f = out(reg) frequency,
            options(nomem, nostack)
        );
    }
    (count, frequency)
}

#[cfg(target_arch = "x86_64")]
fn read() -> (u64, u64) {
    // SAFETY: `rdtsc` only reads the time-stamp counter.
    (unsafe { core::arch::x86_64::_rdtsc() }, 1)
}

fn main() -> i32 {
    let (first, frequency) = read();
    let (second, _) = read();
    check(first > 0 && frequency > 0, "counter reads zero");
    check(second >= first, "counter went back");
    print(b"ticks ok\n");
    0
}

rhizo_cells::entry!(main);
