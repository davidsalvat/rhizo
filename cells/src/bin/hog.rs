//! Hostile: takes all the memory it can get, 1 MiB at a time with `mmap` and
//! then 64 KiB at a time with `brk`, touching every page, until the kernel
//! refuses; prints `exhausted`, yields once, and exits (returning all of it).
//! With argument `file`, the `mmap` steps map a one-byte file of its own
//! instead, nearly all of each step past the file's end (finding S20).
//! With argument `write`, it writes a file of its own 64 KiB at a time
//! until refused (`ENOSPC` or `ENOMEM`) instead (design D4 of
//! `add-cell-hierarchy`: files count in its memory). With argument `wait`,
//! it starts only once the host gave it a file `/go`; with `hold`, it
//! keeps what it took until it is ended (a reserved root's neighbour).
//! Never run on Linux.
#![no_std]
#![no_main]

use rhizo_cells::{arg, check, nr, print, sleep_ns, syscall, yield_now};

const ENOMEM: isize = -12;
const ENOSPC: isize = -28;
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

/// Writes a file until the kernel refuses.
fn write_file() {
    static CHUNK: [u8; 64 << 10] = [7; 64 << 10];
    let fd = one_byte_file();
    let mut written = 0;
    loop {
        let n = syscall(
            nr::WRITE,
            [fd, CHUNK.as_ptr() as usize, CHUNK.len(), 0, 0, 0],
        );
        if n <= 0 {
            check(
                n == ENOSPC || n == ENOMEM,
                "write fails with ENOSPC or ENOMEM",
            );
            break;
        }
        written += n;
    }
    check(written > 0, "some data was written");
}

/// Waits until the host gives the cell a file `/go`.
fn wait_for_go() {
    let path = c"/go".as_ptr() as usize;
    loop {
        let fd = syscall(nr::OPENAT, [AT_FDCWD, path, 0, 0, 0, 0]);
        if fd >= 0 {
            syscall(nr::CLOSE, [fd as usize, 0, 0, 0, 0, 0]);
            return;
        }
        sleep_ns(1_000_000);
    }
}

fn main() -> i32 {
    if arg(1) == Some(b"wait") {
        wait_for_go();
    }
    if arg(1) == Some(b"write") {
        write_file();
        print(b"exhausted\n");
        yield_now();
        return 0;
    }
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
    while arg(1) == Some(b"hold") {
        sleep_ns(3_600_000_000_000);
    }
    yield_now();
    0
}

rhizo_cells::entry!(main);
