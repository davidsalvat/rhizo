//! Holds written memory for the suspend/resume and fork benchmarks: maps
//! argument 2 MiB (default 8), fills it, prints `filled`, yields; then reads
//! back every page (argument 1 `all`, the default), only the first one
//! (`one`), or writes every page (`write`), prints `checked`, yields, and
//! exits.
#![no_std]
#![no_main]

use rhizo_cells::{arg, check, nr, print, syscall, yield_now};

fn main() -> i32 {
    let mib = arg(2).map_or(8, |a| {
        a.iter().fold(0, |n, &d| n * 10 + (d - b'0') as usize)
    });
    let size = mib << 20;
    let p = syscall(nr::MMAP, [0, size, 3, 0x22, usize::MAX, 0]);
    check(p > 0, "mmap");
    let words = p as *mut u64;
    for page in 0..size / 4096 {
        // SAFETY: inside the mapping.
        unsafe { words.add(page * 512).write_volatile(page as u64 + 1) };
    }
    print(b"filled\n");
    yield_now();
    let mode = arg(1).unwrap_or(b"all");
    let pages = if mode == b"one" { 1 } else { size / 4096 };
    for page in 0..pages {
        // SAFETY: inside the mapping.
        let w = unsafe { words.add(page * 512) };
        // SAFETY: as above.
        let v = unsafe { w.read_volatile() };
        check(v == page as u64 + 1, "page contents");
        if mode == b"write" {
            // SAFETY: as above.
            unsafe { w.write_volatile(v + 1) };
        }
    }
    print(b"checked\n");
    yield_now();
    0
}

rhizo_cells::entry!(main);
