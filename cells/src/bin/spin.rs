//! Spins without a system call: only preemption can stop it. Forever, or,
//! given `N`, for `N` million rounds before it exits with 0 (CPU-bound work
//! for the multi-core tests and benchmarks).
#![no_std]
#![no_main]

use rhizo_cells::arg;

fn main() -> i32 {
    let rounds = arg(1).map(|n| {
        n.iter()
            .fold(0u64, |v, d| v * 10 + d.wrapping_sub(b'0') as u64 % 10)
            * 1_000_000
    });
    let mut n: u64 = 0;
    loop {
        // SAFETY: a volatile write to a local keeps the loop from being
        // optimized away.
        unsafe { core::ptr::write_volatile(&mut n, n.wrapping_add(1)) };
        if rounds.is_some_and(|r| n >= r) {
            return 0;
        }
    }
}

rhizo_cells::entry!(main);
