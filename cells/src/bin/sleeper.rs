//! Sleeps 20 ms (or the milliseconds given as its argument) and checks on
//! the monotonic clock that it really did. With `yield` after the
//! milliseconds, it prints `ready`, yields, then sleeps and prints `woken`: the host
//! wakes it before its sleep (`a_wake_before_the_sleep_is_kept`).
#![no_std]
#![no_main]

use rhizo_cells::{arg, check, nr, number, print, sleep_ns, syscall};

fn now_ns() -> u64 {
    let mut ts = [0u64; 2];
    syscall(nr::CLOCK_GETTIME, [1, ts.as_mut_ptr() as usize, 0, 0, 0, 0]);
    ts[0] * 1_000_000_000 + ts[1]
}

fn main() -> i32 {
    let ns = arg(1).map_or(20, number) as u64 * 1_000_000;
    if arg(2) == Some(b"yield") {
        // A wake-up from the host before the sleep must end it.
        print(b"ready\n");
        rhizo_cells::yield_now();
        sleep_ns(ns);
        print(b"woken\n");
        return 0;
    }
    let start = now_ns();
    sleep_ns(ns);
    check(now_ns() - start >= ns, "slept less than asked");
    print(b"slept ok\n");
    0
}

rhizo_cells::entry!(main);
