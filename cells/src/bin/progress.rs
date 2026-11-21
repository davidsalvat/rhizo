//! Works for ever where the host can watch it without stopping it (change
//! `host-ops-per-cell`): prints `at <address>` (hexadecimal) of two words,
//! then, without a system call, adds one to the first in every round and
//! keeps in the second the longest time between two rounds, in ticks of the
//! CPU's counter (as `ticks` reads it). Anything that keeps it off its CPU
//! shows there.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU64, Ordering};
use rhizo_cells::print;

/// Rounds made, and the longest gap between two of them.
static WATCHED: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

#[cfg(target_arch = "aarch64")]
fn ticks() -> u64 {
    let count: u64;
    // SAFETY: reading the virtual counter changes nothing.
    unsafe {
        core::arch::asm!("isb", "mrs {}, cntvct_el0", out(reg) count, options(nomem, nostack))
    };
    count
}

#[cfg(target_arch = "x86_64")]
fn ticks() -> u64 {
    // SAFETY: `rdtsc` only reads the time-stamp counter.
    unsafe { core::arch::x86_64::_rdtsc() }
}

fn main() -> i32 {
    // Written first, so the page is the cell's own before the host looks.
    WATCHED[0].store(1, Ordering::Relaxed);
    let mut line = *b"at 0x0000000000000000\n";
    let addr = WATCHED.as_ptr() as u64;
    for (i, digit) in line[5..21].iter_mut().enumerate() {
        *digit = b"0123456789abcdef"[(addr >> (60 - 4 * i)) as usize & 0xf];
    }
    print(&line);
    let mut last = ticks();
    loop {
        let now = ticks();
        WATCHED[1].fetch_max(now.wrapping_sub(last), Ordering::Relaxed);
        last = now;
        WATCHED[0].fetch_add(1, Ordering::Relaxed);
    }
}

rhizo_cells::entry!(main);
