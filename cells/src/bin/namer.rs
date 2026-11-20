//! Hostile: makes directories with the longest names until the kernel
//! refuses (`ENOSPC`: its limit on files, or memory), then prints
//! `names full` and exits. Never run on Linux.
#![no_std]
#![no_main]

use rhizo_cells::{check, print, syscall};

#[cfg(target_arch = "aarch64")]
const MKDIRAT: usize = 34;
#[cfg(target_arch = "x86_64")]
const MKDIRAT: usize = 258;

const ENOSPC: isize = -28;
const AT_FDCWD: usize = -100isize as usize;

fn main() -> i32 {
    let mut name = [b'n'; 256];
    name[255] = 0;
    for i in 0usize.. {
        // A different name each time: the index in hex at the front.
        for (d, b) in name[..16].iter_mut().enumerate() {
            *b = b"0123456789abcdef"[(i >> (4 * d)) & 15];
        }
        let r = syscall(MKDIRAT, [AT_FDCWD, name.as_ptr() as usize, 0o755, 0, 0, 0]);
        if r != 0 {
            check(r == ENOSPC, "mkdir fails with ENOSPC");
            break;
        }
    }
    print(b"names full\n");
    0
}

rhizo_cells::entry!(main);
