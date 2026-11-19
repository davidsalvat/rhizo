//! Maps one anonymous page (the first mapping, at the start of the mmap
//! area), writes to it, then yields forever.
#![no_std]
#![no_main]

use rhizo_cells::{check, nr, print, syscall, yield_now};

fn main() -> i32 {
    let p = syscall(nr::MMAP, [0, 4096, 3, 0x22, usize::MAX, 0]);
    check(p > 0, "mmap");
    // SAFETY: the page was just mapped.
    unsafe { (p as *mut u64).write_volatile(0x5ec7e7) };
    print(b"mapped\n");
    loop {
        yield_now();
    }
}

rhizo_cells::entry!(main);
