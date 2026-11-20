//! Hostile, endless: calls without pause what used to cost the kernel one
//! step per page of a range, `munmap` and `mprotect` over terabytes of
//! nothing. Other cells must keep running (it is killed by the tests).
#![no_std]
#![no_main]

use rhizo_cells::{nr, print, syscall};

#[cfg(target_arch = "aarch64")]
const MPROTECT: usize = 226;
#[cfg(target_arch = "x86_64")]
const MPROTECT: usize = 10;

fn main() -> i32 {
    print(b"storm\n");
    loop {
        // From 4 GiB (above its image) to below its stack.
        syscall(nr::MUNMAP, [0x1_0000_0000, 0x7ff0_0000_0000, 0, 0, 0, 0]);
        syscall(MPROTECT, [0x4000_0000_0000, 0x3fff_0000_0000, 1, 0, 0, 0]);
    }
}

rhizo_cells::entry!(main);
