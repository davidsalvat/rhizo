//! Makes 20000 `getpid` calls, the cheapest system call, and prints the
//! time per call in nanoseconds (`syscall_ns=N`), measured with
//! `clock_gettime`: the round trip into the kernel and back (the
//! `cell.syscall_ns` benchmark, with and without KPTI).
#![no_std]
#![no_main]

use rhizo_cells::{now_ns, nr, print, syscall};

const CALLS: u64 = 20_000;

fn main() -> i32 {
    let t0 = now_ns();
    for _ in 0..CALLS {
        syscall(nr::GETPID, [0; 6]);
    }
    let ns = (now_ns() - t0) / CALLS;
    let mut line = *b"syscall_ns=00000000\n";
    let mut v = ns.min(99_999_999);
    for at in (11..19).rev() {
        line[at] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    print(&line);
    0
}

rhizo_cells::entry!(main);
