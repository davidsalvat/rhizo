//! Hostile through the page cache (spec `isolation`, finding S14): writes
//! one byte to a file of its own, then maps the file's first page 70000
//! times, more references to one frame than the kernel counts in 16 bits.
//! Every mapping must read the byte. Prints `shares ok`. (Linux refuses
//! mappings past `vm.max_map_count`; this cell runs only in the kernel.)
#![no_std]
#![no_main]

use rhizo_cells::{check, nr, print, syscall};

const AT_FDCWD: usize = -100isize as usize;
const O_RDWR_CREAT: usize = 0o102;
const PROT_READ: usize = 1;
const MAP_PRIVATE: usize = 2;
const MAPPINGS: usize = 70_000;

fn main() -> i32 {
    let fd = syscall(
        nr::OPENAT,
        [AT_FDCWD, c"/shared".as_ptr() as usize, O_RDWR_CREAT, 0o600, 0, 0],
    );
    check(fd >= 0, "open");
    let fd = fd as usize;
    check(
        syscall(nr::PWRITE64, [fd, b"!".as_ptr() as usize, 1, 0, 0, 0]) == 1,
        "write",
    );
    for _ in 0..MAPPINGS {
        let at = syscall(nr::MMAP, [0, 4096, PROT_READ, MAP_PRIVATE, fd, 0]);
        check(at > 0, "mmap");
        // SAFETY: a readable page just mapped.
        let byte = unsafe { (at as *const u8).read_volatile() };
        check(byte == b'!', "the file's byte");
    }
    print(b"shares ok\n");
    0
}

rhizo_cells::entry!(main);
