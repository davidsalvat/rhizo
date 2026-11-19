//! Prints a greeting and exits with status 0.
#![no_std]
#![no_main]

fn main() -> i32 {
    rhizo_cells::print(b"hello from a cell\n");
    0
}

rhizo_cells::entry!(main);
