//! Reads kernel memory (the direct map): the cell must be killed.
#![no_std]
#![no_main]

fn main() -> i32 {
    // SAFETY: deliberately invalid; the fault ends the program.
    let v = unsafe { core::ptr::read_volatile(0xffff_8000_0000_0000 as *const u64) };
    v as i32
}

rhizo_cells::entry!(main);
