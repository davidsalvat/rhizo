//! Holds 8 MiB of written memory for the suspend/resume benchmarks: fills
//! it, prints `filled`, yields; then reads back every page (argument `all`,
//! the default) or only the first one (`one`), prints `checked`, yields, and
//! exits.
#![no_std]
#![no_main]

use rhizo_cells::{arg, check, nr, print, syscall, yield_now};

const SIZE: usize = 8 << 20;

fn main() -> i32 {
    let p = syscall(nr::MMAP, [0, SIZE, 3, 0x22, usize::MAX, 0]);
    check(p > 0, "mmap");
    let words = p as *mut u64;
    for page in 0..SIZE / 4096 {
        // SAFETY: inside the mapping.
        unsafe { words.add(page * 512).write_volatile(page as u64 + 1) };
    }
    print(b"filled\n");
    yield_now();
    let pages = if arg(1) == Some(b"one") {
        1
    } else {
        SIZE / 4096
    };
    for page in 0..pages {
        // SAFETY: inside the mapping.
        let v = unsafe { words.add(page * 512).read_volatile() };
        check(v == page as u64 + 1, "page contents");
    }
    print(b"checked\n");
    yield_now();
    0
}

rhizo_cells::entry!(main);
