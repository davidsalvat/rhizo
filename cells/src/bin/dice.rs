//! Draws 16 random bytes, prints `ready` and yields: the host forks,
//! freezes or suspends it there. Then draws 16 more, checks they differ
//! from the first, and prints them in hex (spec `isolation`: no two cells,
//! forks or resumes draw the same bytes).
#![no_std]
#![no_main]

use rhizo_cells::{check, nr, print, syscall, yield_now};

fn draw() -> [u8; 16] {
    let mut bytes = [0u8; 16];
    let n = syscall(nr::GETRANDOM, [bytes.as_mut_ptr() as usize, 16, 0, 0, 0, 0]);
    check(n == 16, "getrandom");
    bytes
}

fn main() -> i32 {
    let first = draw();
    print(b"ready\n");
    yield_now();
    let second = draw();
    check(first != second, "the same bytes twice");
    let mut line = [b'\n'; 33];
    for (i, byte) in second.iter().enumerate() {
        line[2 * i] = b"0123456789abcdef"[(byte >> 4) as usize];
        line[2 * i + 1] = b"0123456789abcdef"[(byte & 15) as usize];
    }
    print(&line);
    0
}

rhizo_cells::entry!(main);
