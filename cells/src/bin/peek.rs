//! Reads the start of the mmap area without mapping anything: another cell
//! may have mapped it in its own space, never in this one.
#![no_std]
#![no_main]

fn main() -> i32 {
    // SAFETY: deliberately invalid; the fault ends the program.
    let v = unsafe { core::ptr::read_volatile(0x0000_4000_0000_0000 as *const u64) };
    v as i32
}

rhizo_cells::entry!(main);
