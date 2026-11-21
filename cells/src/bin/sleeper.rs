//! Sleeps 20 ms (or the milliseconds given as its argument) and checks on
//! the monotonic clock that it really did. With `yield` after the
//! milliseconds, it prints `ready`, yields, then sleeps and prints `woken`: the host
//! wakes it before its sleep (`a_wake_before_the_sleep_is_kept`). With
//! `epoll`, it prints `ready`, waits that long on an empty `epoll`
//! instance, as libuv waits for its next timer, and prints `timed out`
//! (`a_wait_on_descriptors_keeps_no_time`).
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
    if arg(2) == Some(b"epoll") {
        let epfd = syscall(nr::EPOLL_CREATE1, [0; 6]);
        check(epfd >= 0, "epoll_create1");
        print(b"ready\n");
        let mut event = [0u8; 16];
        let ms = (ns / 1_000_000) as usize;
        let n = syscall(
            nr::EPOLL_PWAIT,
            [epfd as usize, event.as_mut_ptr() as usize, 1, ms, 0, 8],
        );
        check(n == 0, "epoll_pwait");
        print(b"timed out\n");
        return 0;
    }
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
