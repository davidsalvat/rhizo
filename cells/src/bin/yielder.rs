//! Calls `sched_yield` 2000 times and exits: two of them measure the cost
//! of switching between cells (`cell.switch_ns`). With arguments `getpid
//! <n>`, calls `getpid` `n` times instead: the system call round trip
//! (`cell.syscall_ns`).
#![no_std]
#![no_main]

use rhizo_cells::{arg, nr, number, syscall};

fn main() -> i32 {
    if arg(1) == Some(b"getpid") {
        for _ in 0..arg(2).map_or(0, number) {
            syscall(nr::GETPID, [0; 6]);
        }
        return 0;
    }
    for _ in 0..2000 {
        rhizo_cells::yield_now();
    }
    0
}

rhizo_cells::entry!(main);
