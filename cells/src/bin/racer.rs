//! Hostile across CPUs (spec `isolation`, `multi-core`): threads of one
//! cell race on several CPUs to make the kernel use memory, a wait or the
//! cell itself while another thread takes it away. Whatever the kernel
//! answers each thread, it must keep running, keep other cells' memory, and
//! get every frame back. The first argument picks the race:
//!
//! - `io`: three threads copy a page in and out of the kernel (`write` and
//!   `read` on a pipe, `getrandom`) while the first unmaps it and maps a
//!   fresh one in its place, 2000 times. Prints `io ok`.
//! - `futex`: a thread waits on a word of a page the first thread then
//!   unmaps; woken, its next wait there gets `EFAULT`. Prints `futex ok`.
//! - `exit`: three threads make system calls without end while the first
//!   ends the cell with `exit_group(3)`.
//! - `protect`: a thread writes a page while the first makes it read-only
//!   and writable again, 2000 times: the cell may end with a fault, or
//!   print `protect ok`.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use rhizo_cells::{arg, check, exit, join, nr, print, spawn_thread, syscall};

const STACK: usize = 16 * 1024;
const THREADS: usize = 3;
const PAGE: usize = 4096;
const ROUNDS: usize = 2000;

#[cfg(target_arch = "aarch64")]
const MPROTECT: usize = 226;
#[cfg(target_arch = "x86_64")]
const MPROTECT: usize = 10;

#[repr(C, align(16))]
struct Stacks([[u8; STACK]; THREADS]);

static mut STACKS: Stacks = Stacks([[0; STACK]; THREADS]);

/// One identifier word per thread, cleared by the kernel at exit.
static TIDS: [AtomicU32; THREADS] = [const { AtomicU32::new(0) }; THREADS];
static STOP: AtomicU32 = AtomicU32::new(0);
static STARTED: AtomicU32 = AtomicU32::new(0);
static PAGE_AT: AtomicUsize = AtomicUsize::new(0);
static PIPE: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];

fn start(i: usize, f: extern "C" fn(usize) -> usize) {
    // SAFETY: only the address is taken; each stack belongs to one thread.
    let top = unsafe { core::ptr::addr_of_mut!(STACKS.0[i]) as usize + STACK };
    check(spawn_thread(f, i, top, 0, &TIDS[i]) > 0, "clone");
}

fn wait_started(n: u32) {
    while STARTED.load(Ordering::Acquire) < n {
        syscall(nr::SCHED_YIELD, [0; 6]);
    }
}

/// A page mapped at `at` (anywhere if 0, else in place of what is there).
fn map(at: usize) -> usize {
    const MAP_FIXED: usize = 0x10;
    let flags = 0x22 | if at == 0 { 0 } else { MAP_FIXED };
    let page = syscall(nr::MMAP, [at, PAGE, 3, flags, usize::MAX, 0]);
    check(page > 0, "mmap");
    page as usize
}

fn unmap(at: usize) {
    check(syscall(nr::MUNMAP, [at, PAGE, 0, 0, 0, 0]) == 0, "munmap");
}

extern "C" fn copier(i: usize) -> usize {
    STARTED.fetch_add(1, Ordering::Release);
    let page = PAGE_AT.load(Ordering::Acquire);
    let (r, w) = (
        PIPE[0].load(Ordering::Acquire),
        PIPE[1].load(Ordering::Acquire),
    );
    while STOP.load(Ordering::Acquire) == 0 {
        // Any answer will do (bytes, EFAULT, EAGAIN): the kernel must not
        // touch a frame it gave back.
        match i {
            0 => syscall(nr::WRITE, [w, page, PAGE, 0, 0, 0]),
            1 => syscall(nr::READ, [r, page, PAGE, 0, 0, 0]),
            _ => syscall(nr::GETRANDOM, [page, PAGE, 0, 0, 0, 0]),
        };
    }
    0
}

fn io() -> i32 {
    let page = map(0);
    PAGE_AT.store(page, Ordering::Release);
    let mut fds = [0u32; 2];
    const O_NONBLOCK: usize = 0x800;
    check(
        syscall(
            nr::PIPE2,
            [fds.as_mut_ptr() as usize, O_NONBLOCK, 0, 0, 0, 0],
        ) == 0,
        "pipe2",
    );
    PIPE[0].store(fds[0] as usize, Ordering::Release);
    PIPE[1].store(fds[1] as usize, Ordering::Release);
    for i in 0..THREADS {
        start(i, copier);
    }
    wait_started(THREADS as u32);
    for _ in 0..ROUNDS {
        unmap(page);
        check(map(page) == page, "mapped in place");
    }
    STOP.store(1, Ordering::Release);
    for t in &TIDS {
        join(t);
    }
    print(b"io ok\n");
    0
}

static WORD_AT: AtomicUsize = AtomicUsize::new(0);
static WOKEN: AtomicU32 = AtomicU32::new(0);
const EFAULT: isize = -14;
const FUTEX_WAIT: usize = 128;
const FUTEX_WAKE: usize = 129;

extern "C" fn waiter(_: usize) -> usize {
    let word = WORD_AT.load(Ordering::Acquire);
    STARTED.fetch_add(1, Ordering::Release);
    // Waits while the word holds 0 (it does until the page goes).
    syscall(nr::FUTEX, [word, FUTEX_WAIT, 0, 0, 0, 0]);
    WOKEN.store(1, Ordering::Release);
    let again = syscall(nr::FUTEX, [word, FUTEX_WAIT, 0, 0, 0, 0]);
    check(again == EFAULT, "a wait on an unmapped word faults");
    0
}

fn futex() -> i32 {
    let page = map(0);
    WORD_AT.store(page + 64, Ordering::Release);
    start(0, waiter);
    wait_started(1);
    // Let it reach its wait (or not: either way it must end well).
    for _ in 0..100 {
        syscall(nr::SCHED_YIELD, [0; 6]);
    }
    unmap(page);
    while WOKEN.load(Ordering::Acquire) == 0 {
        syscall(nr::FUTEX, [page + 64, FUTEX_WAKE, 1, 0, 0, 0]);
        syscall(nr::SCHED_YIELD, [0; 6]);
    }
    join(&TIDS[0]);
    print(b"futex ok\n");
    0
}

extern "C" fn busy(i: usize) -> usize {
    STARTED.fetch_add(1, Ordering::Release);
    let page = PAGE_AT.load(Ordering::Acquire);
    loop {
        match i {
            0 => syscall(nr::GETPID, [0; 6]),
            1 => syscall(nr::GETRANDOM, [page, PAGE, 0, 0, 0, 0]),
            _ => syscall(nr::SCHED_YIELD, [0; 6]),
        };
    }
}

fn exit_busy() -> i32 {
    PAGE_AT.store(map(0), Ordering::Release);
    for i in 0..THREADS {
        start(i, busy);
    }
    wait_started(THREADS as u32);
    exit(3)
}

extern "C" fn writer(_: usize) -> usize {
    let page = PAGE_AT.load(Ordering::Acquire) as *mut u64;
    STARTED.fetch_add(1, Ordering::Release);
    let mut n = 0u64;
    while STOP.load(Ordering::Acquire) == 0 {
        n += 1;
        // SAFETY: the page stays mapped; while it is read-only the write
        // faults and ends the cell, which the test allows.
        unsafe { page.write_volatile(n) };
    }
    0
}

fn protect() -> i32 {
    let page = map(0);
    PAGE_AT.store(page, Ordering::Release);
    start(0, writer);
    wait_started(1);
    for _ in 0..ROUNDS {
        for prot in [1, 3] {
            check(
                syscall(MPROTECT, [page, PAGE, prot, 0, 0, 0]) == 0,
                "mprotect",
            );
        }
    }
    STOP.store(1, Ordering::Release);
    join(&TIDS[0]);
    print(b"protect ok\n");
    0
}

fn main() -> i32 {
    match arg(1) {
        Some(b"io") => io(),
        Some(b"futex") => futex(),
        Some(b"exit") => exit_busy(),
        Some(b"protect") => protect(),
        _ => {
            print(b"usage: racer io|futex|exit|protect\n");
            2
        }
    }
}

rhizo_cells::entry!(main);
