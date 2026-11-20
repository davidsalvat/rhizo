//! Writes one byte into a pipe and reads it back, N times (the first
//! argument, 1000 by default), then prints `pipes ok`. Used by
//! `cell.pipe_rtt_ns`.
#![no_std]
#![no_main]

use rhizo_cells::{arg, check, nr, number, print, syscall};

fn main() -> i32 {
    let n = arg(1).map_or(1000, number);
    let mut fds = [0i32; 2];
    check(
        syscall(nr::PIPE2, [fds.as_mut_ptr() as usize, 0, 0, 0, 0, 0]) == 0,
        "pipe2",
    );
    let (r, w) = (fds[0] as usize, fds[1] as usize);
    let mut byte = [0u8; 1];
    for i in 0..n {
        let out = [i as u8];
        check(
            syscall(nr::WRITE, [w, out.as_ptr() as usize, 1, 0, 0, 0]) == 1,
            "write",
        );
        check(
            syscall(nr::READ, [r, byte.as_mut_ptr() as usize, 1, 0, 0, 0]) == 1 && byte == out,
            "read",
        );
    }
    print(b"pipes ok\n");
    0
}

rhizo_cells::entry!(main);
