//! Writes to address 0: the cell must be killed, the kernel must go on.
#![no_std]
#![no_main]

fn main() -> i32 {
    // SAFETY: deliberately invalid; the fault ends the program.
    unsafe { core::ptr::write_volatile(core::ptr::null_mut::<u64>(), 1) };
    0
}

rhizo_cells::entry!(main);
