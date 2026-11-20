//! A page unmapped under a reader (spec `multi-core`, TLB coherence): one
//! thread keeps reading a page while the first thread unmaps it and then
//! raises a flag. The reader looks at the flag before each read, so a read
//! that succeeds after it saw the flag used a stale translation: it prints
//! `stale read` and exits with 1. Otherwise its next read faults and the
//! cell ends with the fault (on Linux: `SIGSEGV`), printing nothing.
//!
//! With `bench [rounds]` it measures instead (design D9 of
//! `add-parallel-threads`): three threads spin, writing their own memory,
//! while the first maps, writes and unmaps one page `rounds` times (200 by
//! default), and it prints `shootdown_ns=<median munmap>`.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use rhizo_cells::{arg, check, exit, join, now_ns, nr, number, print, spawn_thread, syscall};

const STACK: usize = 16 * 1024;
const SPINNERS: usize = 3;
const PAGE: usize = 4096;
const MAX_ROUNDS: usize = 1000;

#[repr(C, align(16))]
struct Stacks([[u8; STACK]; SPINNERS]);

static mut STACKS: Stacks = Stacks([[0; STACK]; SPINNERS]);

/// The top of stack `i`.
fn stack(i: usize) -> usize {
    // SAFETY: only the address is taken; each stack belongs to one thread.
    unsafe { core::ptr::addr_of_mut!(STACKS.0[i]) as usize + STACK }
}

/// One identifier word per thread, cleared by the kernel at exit.
static TIDS: [AtomicU32; SPINNERS] = [const { AtomicU32::new(0) }; SPINNERS];

fn start(i: usize, f: extern "C" fn(usize) -> usize) {
    check(spawn_thread(f, i, stack(i), 0, &TIDS[i]) > 0, "clone");
}

/// A new private, anonymous, readable and writable page.
fn map_page() -> usize {
    let page = syscall(nr::MMAP, [0, PAGE, 3, 0x22, usize::MAX, 0]);
    check(page > 0, "mmap");
    page as usize
}

fn unmap(page: usize) {
    check(syscall(nr::MUNMAP, [page, PAGE, 0, 0, 0, 0]) == 0, "munmap");
}

// --- the test ----------------------------------------------------------------

static PAGE_ADDR: AtomicUsize = AtomicUsize::new(0);
static GONE: AtomicU32 = AtomicU32::new(0);
static READS: AtomicU64 = AtomicU64::new(0);
const MARK: u64 = 0x5a5a_5a5a_5a5a_5a5a;

extern "C" fn reader(_: usize) -> usize {
    let page = PAGE_ADDR.load(Ordering::Acquire) as *const u64;
    loop {
        let gone = GONE.load(Ordering::Acquire);
        // SAFETY: the page is mapped until the first thread unmaps it; a
        // read after that must fault, which is what is tested.
        let value = unsafe { core::ptr::read_volatile(page) };
        if gone != 0 {
            print(b"stale read\n");
            exit(1);
        }
        check(value == MARK, "the page holds what was written");
        READS.fetch_add(1, Ordering::Relaxed);
    }
}

fn test() -> i32 {
    let page = map_page();
    // SAFETY: the page was just mapped readable and writable.
    unsafe { core::ptr::write_volatile(page as *mut u64, MARK) };
    PAGE_ADDR.store(page, Ordering::Release);
    start(0, reader);
    // Wait until the reader is well into its loop (with one CPU, it runs
    // when this thread yields).
    while READS.load(Ordering::Relaxed) < 10_000 {
        syscall(nr::SCHED_YIELD, [0; 6]);
    }
    unmap(page);
    GONE.store(1, Ordering::Release);
    // The reader faults and ends the cell.
    loop {
        syscall(nr::SCHED_YIELD, [0; 6]);
    }
}

// --- the benchmark -----------------------------------------------------------

static STOP: AtomicU32 = AtomicU32::new(0);
static SPINNING: AtomicU32 = AtomicU32::new(0);
static SPUN: [AtomicU64; SPINNERS] = [const { AtomicU64::new(0) }; SPINNERS];

extern "C" fn spinner(i: usize) -> usize {
    SPINNING.fetch_add(1, Ordering::Relaxed);
    while STOP.load(Ordering::Relaxed) == 0 {
        SPUN[i].fetch_add(1, Ordering::Relaxed);
    }
    0
}

fn bench(rounds: usize) -> i32 {
    let rounds = rounds.clamp(1, MAX_ROUNDS);
    for i in 0..SPINNERS {
        start(i, spinner);
    }
    while SPINNING.load(Ordering::Relaxed) < SPINNERS as u32 {
        syscall(nr::SCHED_YIELD, [0; 6]);
    }
    let mut samples = [0u64; MAX_ROUNDS];
    for sample in &mut samples[..rounds] {
        let page = map_page();
        // SAFETY: the page was just mapped readable and writable.
        unsafe { core::ptr::write_volatile(page as *mut u64, MARK) };
        let t0 = now_ns();
        unmap(page);
        *sample = now_ns() - t0;
    }
    STOP.store(1, Ordering::Relaxed);
    for t in &TIDS {
        join(t);
    }
    let samples = &mut samples[..rounds];
    samples.sort_unstable();
    let median = samples[rounds / 2];
    let mut text = *b"shootdown_ns=00000000000000000000\n";
    let mut n = median;
    for d in text[13..33].iter_mut().rev() {
        *d = b'0' + (n % 10) as u8;
        n /= 10;
    }
    let first = text[13..32].iter().position(|&d| d != b'0').unwrap_or(19);
    print(&text[..13]);
    print(&text[13 + first..]);
    0
}

fn main() -> i32 {
    match arg(1) {
        Some(b"bench") => bench(arg(2).map_or(200, number)),
        _ => test(),
    }
}

rhizo_cells::entry!(main);
