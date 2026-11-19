//! Calls `sched_yield` 2000 times and exits: two of them measure the cost
//! of switching between cells (`cell.switch_ns`).
#![no_std]
#![no_main]

fn main() -> i32 {
    for _ in 0..2000 {
        rhizo_cells::yield_now();
    }
    0
}

rhizo_cells::entry!(main);
