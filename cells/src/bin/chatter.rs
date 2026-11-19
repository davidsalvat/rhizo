//! Prints its tag (argument 1, default `x`) three times, yielding between
//! lines, so several of them interleave.
#![no_std]
#![no_main]

use rhizo_cells::{arg, print, yield_now};

fn main() -> i32 {
    let tag = arg(1).unwrap_or(b"x");
    for _ in 0..3 {
        print(tag);
        print(b"\n");
        yield_now();
    }
    0
}

rhizo_cells::entry!(main);
