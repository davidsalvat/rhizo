//! Hostile: takes all the memory it can get, 1 MiB at a time with `mmap` and
//! then 64 KiB at a time with `brk`, touching every page, until the kernel
//! refuses; prints `exhausted`, yields once, and exits (returning all of it).
//! With argument `file`, the `mmap` steps map a one-byte file of its own
//! instead, nearly all of each step past the file's end (finding S20).
//! Never run on Linux.
#![no_std]
#![no_main]

use rhizo_cells::{arg, check, nr, print, syscall, yield_now};

const ENOMEM: isize = -12;
const AT_FDCWD: usize = -100isize as usize;
const O_RDWR_CREAT: usize = 0o102;

/// A one-byte file of the cell's own, open.
fn one_byte_file() -> usize {
    let path = c"/hog".as_ptr() as usize;
    let fd = syscall(nr::OPENAT, [AT_FDCWD, path, O_RDWR_CREAT, 0o600, 0, 0]);
    check(fd >= 0, "open");
    let fd = fd as usize;
    check(
        syscall(nr::PWRITE64, [fd, b"!".as_ptr() as usize, 1, 0, 0, 0]) == 1,
        "write",
    );
    fd
}

fn touch(start: usize, len: usize) {
    for page in (start..start + len).step_by(4096) {
        // SAFETY: the kernel just mapped `start..start + len` for this cell.
        unsafe { (page as *mut u64).write_volatile(page as u64) };
    }
}

fn main() -> i32 {
    let file = (arg(1) == Some(b"file")).then(one_byte_file);
    let mut mapped = 0;
    loop {
        // Read-only and private for the file: its pages are mapped at once.
        let p = match file {
            Some(fd) => syscall(nr::MMAP, [0, 1 << 20, 1, 0x02, fd, 0]),
            None => syscall(nr::MMAP, [0, 1 << 20, 3, 0x22, usize::MAX, 0]),
        };
        if p < 0 {
            check(p == ENOMEM, "mmap fails with ENOMEM");
            break;
        }
        if file.is_none() {
            touch(p as usize, 1 << 20);
        }
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
