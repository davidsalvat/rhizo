//! Hostile: writes kernel memory (the direct map): the cell must be killed
//! with a fault report, as reading it is.
#![no_std]
#![no_main]

fn main() -> i32 {
    // SAFETY: deliberately invalid; the fault ends the program.
    unsafe { core::ptr::write_volatile(0xffff_8000_0000_0000 as *mut u64, 0x5eed) };
    0
}

rhizo_cells::entry!(main);
