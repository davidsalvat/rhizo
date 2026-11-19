//! Writes a file of N MiB (the first argument, 1 by default) in 64 KiB
//! writes, reads it back and checks every byte, prints `files ok` and
//! yields (the benchmark forks it there, holding its file), then exits.
//! Used by `cell.file_write_read_us` and `cell.fork_with_files_us`.
#![no_std]
#![no_main]

use rhizo_cells::{arg, check, nr, print, syscall, yield_now};

const AT_FDCWD: usize = -100isize as usize;
const CHUNK: usize = 64 << 10;

static mut BUF: [u8; CHUNK] = [0; CHUNK];

fn main() -> i32 {
    let mib = arg(1)
        .map(|a| a.iter().fold(0, |n, d| n * 10 + (d - b'0') as usize))
        .unwrap_or(1);
    // SAFETY: single-threaded program; the only reference to BUF.
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(BUF) };
    let path = b"data.bin\0";
    let flags = 0x2 | 0x40 | 0x200; // O_RDWR | O_CREAT | O_TRUNC
    let fd = syscall(
        nr::OPENAT,
        [AT_FDCWD, path.as_ptr() as usize, flags, 0o644, 0, 0],
    );
    check(fd >= 0, "open");
    let fd = fd as usize;
    let chunks = mib * (1 << 20) / CHUNK;
    for c in 0..chunks {
        buf.fill(c as u8);
        let n = syscall(nr::WRITE, [fd, buf.as_ptr() as usize, CHUNK, 0, 0, 0]);
        check(n == CHUNK as isize, "write");
    }
    for c in 0..chunks {
        let at = c * CHUNK;
        let n = syscall(
            nr::PREAD64,
            [fd, buf.as_mut_ptr() as usize, CHUNK, at, 0, 0],
        );
        check(n == CHUNK as isize, "read");
        check(buf.iter().all(|&b| b == c as u8), "data");
    }
    print(b"files ok\n");
    yield_now();
    0
}

rhizo_cells::entry!(main);
