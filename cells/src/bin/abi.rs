//! Exercises the H2 system calls and prints one line per check group.
#![no_std]
#![no_main]

use rhizo_cells::{check, nr, print, syscall};

const EFAULT: isize = -14;
const ENOSYS: isize = -38;

fn main() -> i32 {
    // brk: grow by three pages, the new memory is zero and writable.
    let start = syscall(nr::BRK, [0; 6]) as usize;
    let end = start + 3 * 4096;
    check(
        syscall(nr::BRK, [end, 0, 0, 0, 0, 0]) as usize == end,
        "brk grow",
    );
    let heap = start as *mut u8;
    // SAFETY: [start, end) was just added to the heap.
    unsafe {
        check(*heap.add(3 * 4096 - 1) == 0, "brk zero");
        heap.write_volatile(7);
        check(heap.read_volatile() == 7, "brk write");
    }
    print(b"brk ok\n");

    // mmap: 4 MiB of anonymous memory, zero, writable at both ends.
    let len = 4 << 20;
    let p = syscall(nr::MMAP, [0, len, 3, 0x22, usize::MAX, 0]);
    check(p > 0, "mmap");
    let p = p as *mut u64;
    // SAFETY: the mapping covers [p, p + len).
    unsafe {
        let last = p.add(len / 8 - 1);
        check(last.read_volatile() == 0, "mmap zero");
        p.write_volatile(1);
        last.write_volatile(2);
    }
    check(
        syscall(nr::MUNMAP, [p as usize, len, 0, 0, 0, 0]) == 0,
        "munmap",
    );
    print(b"mmap ok\n");

    // Bad pointers return EFAULT; the program keeps running.
    check(syscall(nr::WRITE, [1, 8, 4, 0, 0, 0]) == EFAULT, "efault");
    print(b"efault ok\n");

    // An unknown call returns ENOSYS.
    check(syscall(999, [0; 6]) == ENOSYS, "enosys");
    print(b"enosys ok\n");

    // uname, clock_gettime, getrandom, getpid succeed.
    let mut uts = [0u8; 6 * 65];
    check(
        syscall(nr::UNAME, [uts.as_mut_ptr() as usize, 0, 0, 0, 0, 0]) == 0,
        "uname",
    );
    check(&uts[..6] == b"Linux\0", "uname sysname");
    let mut ts = [0u64; 2];
    check(
        syscall(nr::CLOCK_GETTIME, [1, ts.as_mut_ptr() as usize, 0, 0, 0, 0]) == 0,
        "clock",
    );
    check(ts[1] < 1_000_000_000, "clock nsec");
    let mut buf = [0u8; 32];
    check(
        syscall(nr::GETRANDOM, [buf.as_mut_ptr() as usize, 32, 0, 0, 0, 0]) == 32,
        "getrandom",
    );
    check(syscall(nr::GETPID, [0; 6]) > 0, "getpid");
    print(b"misc ok\n");
    0
}

rhizo_cells::entry!(main);
