//! Prints `idle` and sleeps an hour at a time, forever: the idle cell the
//! density test and `cell.idle_bytes` fork by the thousand.
#![no_std]
#![no_main]

use rhizo_cells::{print, sleep_ns};

fn main() -> i32 {
    print(b"idle\n");
    loop {
        sleep_ns(3_600_000_000_000);
    }
}

rhizo_cells::entry!(main);
