//! Hostile: takes all the memory it can get, 1 MiB at a time with `mmap` and
//! then 64 KiB at a time with `brk`, touching every page, until the kernel
//! refuses; prints `exhausted`, yields once, and exits (returning all of it).
//! Never run on Linux.
#![no_std]
#![no_main]

use rhizo_cells::{check, nr, print, syscall, yield_now};

const ENOMEM: isize = -12;

fn touch(start: usize, len: usize) {
    for page in (start..start + len).step_by(4096) {
        // SAFETY: the kernel just mapped `start..start + len` for this cell.
        unsafe { (page as *mut u64).write_volatile(page as u64) };
    }
}

fn main() -> i32 {
    let mut mapped = 0;
    loop {
        let p = syscall(nr::MMAP, [0, 1 << 20, 3, 0x22, usize::MAX, 0]);
        if p < 0 {
            check(p == ENOMEM, "mmap fails with ENOMEM");
            break;
        }
        touch(p as usize, 1 << 20);
        mapped += 1;
    }
    let start = syscall(nr::BRK, [0; 6]) as usize;
    let mut end = start;
    loop {
        let want = end + (64 << 10);
        if syscall(nr::BRK, [want, 0, 0, 0, 0, 0]) as usize != want {
            break; // the break stays where it was
        }
        touch(end, 64 << 10);
        end = want;
    }
    check(mapped > 0, "some memory was given");
    print(b"exhausted\n");
    yield_now();
    0
}

rhizo_cells::entry!(main);
