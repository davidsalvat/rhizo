//! Sleeps 20 ms and checks on the monotonic clock that it really did.
#![no_std]
#![no_main]

use rhizo_cells::{check, nr, print, sleep_ns, syscall};

fn now_ns() -> u64 {
    let mut ts = [0u64; 2];
    syscall(nr::CLOCK_GETTIME, [1, ts.as_mut_ptr() as usize, 0, 0, 0, 0]);
    ts[0] * 1_000_000_000 + ts[1]
}

fn main() -> i32 {
    let start = now_ns();
    sleep_ns(20_000_000);
    check(now_ns() - start >= 20_000_000, "slept less than asked");
    print(b"slept ok\n");
    0
}

rhizo_cells::entry!(main);
