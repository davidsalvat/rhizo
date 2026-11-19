//! Exits with status 3 without printing.
#![no_std]
#![no_main]

fn main() -> i32 {
    3
}

rhizo_cells::entry!(main);
