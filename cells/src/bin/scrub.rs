//! Checks that memory a cell is given holds nothing of anyone else. With
//! argument `dirty`, it fills 16 MiB with a pattern, prints `dirty` and
//! exits (leaving those frames free for the next cell). Otherwise it prints
//! `ready`, yields (the point where a host may fork or suspend it), then
//! maps 16 MiB and grows its heap by 1 MiB, checks every byte is zero, and
//! prints `zero ok`.
#![no_std]
#![no_main]

use rhizo_cells::{arg, check, nr, print, syscall, yield_now};

const LEN: usize = 16 << 20;
const HEAP: usize = 1 << 20;
const PATTERN: u64 = 0xa5a5_5a5a_dead_beef;

fn words(start: usize, len: usize) -> impl Iterator<Item = *mut u64> {
    (start..start + len).step_by(8).map(|a| a as *mut u64)
}

fn main() -> i32 {
    if arg(1) == Some(b"dirty") {
        let p = syscall(nr::MMAP, [0, LEN, 3, 0x22, usize::MAX, 0]);
        check(p > 0, "mmap");
        for w in words(p as usize, LEN) {
            // SAFETY: inside the mapping.
            unsafe { w.write_volatile(PATTERN) };
        }
        print(b"dirty\n");
        return 0;
    }
    print(b"ready\n");
    yield_now();
    let p = syscall(nr::MMAP, [0, LEN, 3, 0x22, usize::MAX, 0]);
    check(p > 0, "mmap");
    let start = syscall(nr::BRK, [0; 6]) as usize;
    check(
        syscall(nr::BRK, [start + HEAP, 0, 0, 0, 0, 0]) as usize == start + HEAP,
        "brk",
    );
    for (base, len) in [(p as usize, LEN), (start, HEAP)] {
        for w in words(base, len) {
            // SAFETY: inside memory the kernel just gave this cell.
            check(unsafe { w.read_volatile() } == 0, "fresh memory is zero");
        }
    }
    print(b"zero ok\n");
    0
}

rhizo_cells::entry!(main);
