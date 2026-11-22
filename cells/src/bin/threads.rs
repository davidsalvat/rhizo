//! Threads made with raw `clone`, synchronized with raw futexes (spec
//! `threads`): distinct identifiers and thread pointers, joins through the
//! clear-tid word, futex wait and wake with values, timeouts and bitsets, a
//! futex mutex under contention, a ping-pong, and registers kept while
//! another thread of the cell makes system calls. The first thread ends
//! with `exit`, being the last one.
//!
//! With the argument `bench` it measures instead, and prints
//! `create_ns=<n> futex_wake_ns=<n>` (design D10 of `add-threads`). The
//! in-kernel scheduling tests use four more modes: `starve` (a thread
//! prints and ends the cell while the first spins), `spin <n>` (`n`
//! threads spin forever), `park` (every thread waits forever) and `pause`
//! (it prints `ready`, wakes a thread that yields four times and then
//! waits forever, sleeps 20 ms, prints `done` and ends: a template whose
//! pause starts while another of its threads can still run, as Node.js's
//! after `warm`, change `fix-node-pause`).
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use rhizo_cells::{
    THREAD_FLAGS, arg, check, futex_wait, futex_wake, join, now_ns, nr, print, spawn_thread,
    syscall, yield_now,
};

const EAGAIN: isize = -11;
const ETIMEDOUT: isize = -110;
const ANY: u32 = u32::MAX;
const THREADS: usize = 4;
const STACK: usize = 16 * 1024;

#[repr(C, align(16))]
struct Stacks([[u8; STACK]; THREADS]);

static mut STACKS: Stacks = Stacks([[0; STACK]; THREADS]);

/// The top of stack `i`.
fn stack(i: usize) -> usize {
    // SAFETY: only the address is taken; each stack is used by one thread
    // at a time (a thread is joined before its stack is reused).
    unsafe { core::ptr::addr_of_mut!(STACKS.0[i]) as usize + STACK }
}

/// One identifier word per thread slot, cleared by the kernel at exit.
static TIDS: [AtomicU32; THREADS] = [const { AtomicU32::new(0) }; THREADS];

/// Starts `f(i)` as the thread of slot `i`, with thread pointer `tls`.
fn start(i: usize, f: extern "C" fn(usize) -> usize, tls: usize) -> u32 {
    let tid = spawn_thread(f, i, stack(i), tls, &TIDS[i]);
    check(tid > 0, "clone");
    tid as u32
}

/// The calling thread's thread pointer.
fn thread_pointer() -> usize {
    #[cfg(target_arch = "aarch64")]
    {
        let tp: usize;
        // SAFETY: reading TPIDR_EL0 has no side effects.
        unsafe { core::arch::asm!("mrs {}, tpidr_el0", out(reg) tp, options(nomem, nostack)) };
        tp
    }
    #[cfg(target_arch = "x86_64")]
    {
        let mut fs = 0usize;
        const ARCH_GET_FS: usize = 0x1003;
        syscall(
            nr::ARCH_PRCTL,
            [
                ARCH_GET_FS,
                core::ptr::addr_of_mut!(fs) as usize,
                0,
                0,
                0,
                0,
            ],
        );
        fs
    }
}

fn gettid() -> u32 {
    syscall(nr::GETTID, [0; 6]) as u32
}

// --- identifiers and thread pointers ---------------------------------------

static SEEN_TID: [AtomicU32; THREADS] = [const { AtomicU32::new(0) }; THREADS];
static TLS_OK: AtomicU32 = AtomicU32::new(0);

fn tls_of(i: usize) -> usize {
    0x5000 + i * 0x100
}

extern "C" fn identify(i: usize) -> usize {
    SEEN_TID[i].store(gettid(), Ordering::Relaxed);
    for _ in 0..3 {
        // Other threads run in between: each keeps its own thread pointer.
        yield_now();
        if thread_pointer() == tls_of(i) {
            TLS_OK.fetch_add(1, Ordering::Relaxed);
        }
    }
    0
}

fn identifiers() {
    let me = gettid();
    check(
        me as isize == syscall(nr::GETPID, [0; 6]),
        "first tid is the pid",
    );
    let mut tids = [0u32; THREADS];
    for (i, tid) in tids.iter_mut().enumerate() {
        *tid = start(i, identify, tls_of(i));
    }
    for t in &TIDS {
        join(t);
    }
    for i in 0..THREADS {
        check(SEEN_TID[i].load(Ordering::Relaxed) == tids[i], "gettid");
        check(
            tids[i] != me && tids[..i].iter().all(|&t| t != tids[i]),
            "distinct tids",
        );
    }
    check(
        TLS_OK.load(Ordering::Relaxed) == 3 * THREADS as u32,
        "own thread pointers",
    );
    check(
        thread_pointer() != tls_of(0),
        "the first thread's pointer is its own",
    );
    print(b"tids ok\n");
}

// --- futex wait, wake, timeouts and bitsets --------------------------------

static WORD: AtomicU32 = AtomicU32::new(0);
static WOKEN: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];

extern "C" fn wait_bits(i: usize) -> usize {
    while futex_wait(&WORD, 0, None, 1 << i) != 0 {}
    WOKEN[i].store(1, Ordering::Release);
    0
}

/// Wakes waiters with `bitset` until one wakes (it may not wait yet).
fn wake_one(bitset: u32) {
    while futex_wake(&WORD, u32::MAX, bitset) == 0 {
        yield_now();
    }
}

fn futexes() {
    check(
        futex_wait(&WORD, 1, None, ANY) == EAGAIN,
        "value changed: EAGAIN",
    );
    let t0 = now_ns();
    check(
        futex_wait(&WORD, 0, Some(20_000_000), ANY) == ETIMEDOUT,
        "timeout",
    );
    check(now_ns() - t0 >= 20_000_000, "timed out early");
    check(futex_wake(&WORD, 1, ANY) == 0, "nobody waits");
    start(0, wait_bits, 0);
    start(1, wait_bits, 0);
    wake_one(0b10);
    join(&TIDS[1]);
    check(
        WOKEN[0].load(Ordering::Acquire) == 0,
        "bitset 1 is not woken by 2",
    );
    wake_one(0b01);
    join(&TIDS[0]);
    print(b"futex ok\n");
}

// --- a futex mutex under contention ----------------------------------------

/// 0 free, 1 locked, 2 locked with waiters.
static LOCK: AtomicU32 = AtomicU32::new(0);
static COUNTER: AtomicU64 = AtomicU64::new(0);
const INCREMENTS: u64 = 25;

fn lock() {
    if LOCK
        .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
    {
        return;
    }
    while LOCK.swap(2, Ordering::Acquire) != 0 {
        futex_wait(&LOCK, 2, None, ANY);
    }
}

fn unlock() {
    if LOCK.swap(0, Ordering::Release) == 2 {
        futex_wake(&LOCK, 1, ANY);
    }
}

extern "C" fn increment(_: usize) -> usize {
    for _ in 0..INCREMENTS {
        lock();
        // Read, let the others run, write: lost updates without the lock.
        let v = COUNTER.load(Ordering::Relaxed);
        yield_now();
        COUNTER.store(v + 1, Ordering::Relaxed);
        unlock();
    }
    0
}

fn mutex() {
    for i in 0..THREADS {
        start(i, increment, 0);
    }
    for t in &TIDS {
        join(t);
    }
    check(
        COUNTER.load(Ordering::Relaxed) == INCREMENTS * THREADS as u64,
        "lost an update",
    );
    print(b"mutex ok\n");
}

// --- ping-pong -------------------------------------------------------------

/// Whose turn it is: 0 or 1.
static TURN: AtomicU32 = AtomicU32::new(0);
static ROUNDS: AtomicU32 = AtomicU32::new(0);

/// Waits for `me`'s turn, then hands it to the other, `ROUNDS` times.
extern "C" fn ping(me: usize) -> usize {
    let me = me as u32;
    for _ in 0..ROUNDS.load(Ordering::Relaxed) {
        loop {
            let turn = TURN.load(Ordering::Acquire);
            if turn == me {
                break;
            }
            futex_wait(&TURN, turn, None, ANY);
        }
        TURN.store(1 - me, Ordering::Release);
        futex_wake(&TURN, 1, ANY);
    }
    0
}

/// This thread and another hand the turn to each other `rounds` times
/// each; returns the nanoseconds it took.
fn ping_pong(rounds: u32) -> u64 {
    TURN.store(0, Ordering::Relaxed);
    ROUNDS.store(rounds, Ordering::Relaxed);
    start(1, ping, 0);
    let t0 = now_ns();
    ping(0);
    join(&TIDS[1]);
    now_ns() - t0
}

// --- registers across thread switches --------------------------------------

static STOP: AtomicU32 = AtomicU32::new(0);
static REGS_OK: AtomicU32 = AtomicU32::new(0);

/// Holds known values in registers the system-call return path could
/// clobber (RCX and R11 on x86_64) while spinning until `STOP`, so it is
/// preempted while the other thread makes system calls.
extern "C" fn hold_registers(_: usize) -> usize {
    let ok: usize;
    let stop = STOP.as_ptr();
    // SAFETY: only registers declared as clobbered are written; `stop` is a
    // valid `u32` read each round.
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!(
            "mov rcx, 0x1111",
            "mov r11, 0x2222",
            "2:",
            "cmp rcx, 0x1111",
            "jne 3f",
            "cmp r11, 0x2222",
            "jne 3f",
            "cmp dword ptr [{stop}], 0",
            "je 2b",
            "mov {ok}, 1",
            "jmp 4f",
            "3:",
            "mov {ok}, 0",
            "4:",
            stop = in(reg) stop,
            ok = out(reg) ok,
            out("rcx") _, out("r11") _,
            options(nostack)
        );
    }
    // SAFETY: as above.
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!(
            "mov x9, #0x111",
            "mov x10, #0x222",
            "2:",
            "cmp x9, #0x111",
            "b.ne 3f",
            "cmp x10, #0x222",
            "b.ne 3f",
            "ldr w11, [{stop}]",
            "cbz w11, 2b",
            "mov {ok}, #1",
            "b 4f",
            "3:",
            "mov {ok}, #0",
            "4:",
            stop = in(reg) stop,
            ok = out(reg) ok,
            out("x9") _, out("x10") _, out("x11") _,
            options(nostack)
        );
    }
    REGS_OK.store(ok as u32, Ordering::Relaxed);
    0
}

fn registers() {
    start(0, hold_registers, 0);
    // System calls here; the spinner runs (and is preempted) in between.
    let t0 = now_ns();
    while now_ns() - t0 < 50_000_000 {
        yield_now();
    }
    STOP.store(1, Ordering::Relaxed);
    join(&TIDS[0]);
    check(REGS_OK.load(Ordering::Relaxed) == 1, "registers changed");
    print(b"regs ok\n");
}

// --- benchmarks --------------------------------------------------------------

extern "C" fn nothing(_: usize) -> usize {
    0
}

/// Median of each of five rounds of `samples`, then the minimum.
fn robust(samples: &mut [u64]) -> u64 {
    let per = samples.len() / 5;
    samples
        .chunks_mut(per)
        .map(|round| {
            round.sort_unstable();
            round[round.len() / 2]
        })
        .min()
        .unwrap_or(0)
}

fn bench() {
    let mut create = [0u64; 5 * 21];
    for sample in create.iter_mut() {
        let t0 = now_ns();
        start(0, nothing, 0);
        join(&TIDS[0]);
        *sample = now_ns() - t0;
    }
    const ROUNDS: u32 = 200;
    let mut wake = [0u64; 5];
    for sample in wake.iter_mut() {
        *sample = ping_pong(ROUNDS) / (2 * ROUNDS as u64);
    }
    let mut line = [0u8; 64];
    let mut n = 0;
    for (label, value) in [
        (&b"create_ns="[..], robust(&mut create)),
        (b" futex_wake_ns=", wake.iter().copied().min().unwrap_or(0)),
    ] {
        line[n..n + label.len()].copy_from_slice(label);
        n += label.len();
        n += decimal(value, &mut line[n..]);
    }
    line[n] = b'\n';
    print(&line[..=n]);
}

/// Writes `v` in decimal at the start of `out`; returns its length.
fn decimal(mut v: u64, out: &mut [u8]) -> usize {
    let mut digits = [0u8; 20];
    let mut len = 0;
    loop {
        digits[len] = b'0' + (v % 10) as u8;
        len += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    for i in 0..len {
        out[i] = digits[len - 1 - i];
    }
    len
}

// --- scheduling modes --------------------------------------------------------

extern "C" fn spin(_: usize) -> usize {
    loop {
        core::hint::spin_loop();
    }
}

extern "C" fn report_and_exit(_: usize) -> usize {
    print(b"not starved\n");
    rhizo_cells::exit(0)
}

static PARKED: AtomicU32 = AtomicU32::new(0);

extern "C" fn park(_: usize) -> usize {
    loop {
        futex_wait(&PARKED, 0, None, ANY);
    }
}

static GO: AtomicU32 = AtomicU32::new(0);

extern "C" fn settle(_: usize) -> usize {
    while GO.load(Ordering::Acquire) == 0 {
        futex_wait(&GO, 0, None, ANY);
    }
    for _ in 0..4 {
        yield_now();
    }
    park(0)
}

fn main() -> i32 {
    match (arg(1), arg(2)) {
        (Some(b"pause"), _) => {
            start(0, settle, 0);
            print(b"ready\n");
            GO.store(1, Ordering::Release);
            futex_wake(&GO, 1, ANY);
            rhizo_cells::sleep_ns(20_000_000);
            print(b"done\n");
            rhizo_cells::exit(0)
        }
        (Some(b"bench"), _) => {
            bench();
            return 0;
        }
        (Some(b"starve"), _) => {
            start(0, report_and_exit, 0);
            spin(0);
        }
        (Some(b"spin"), Some(&[n])) => {
            for i in 1..(n - b'0') as usize {
                start(i, spin, 0);
            }
            spin(0);
        }
        (Some(b"park"), _) => {
            start(0, park, 0);
            park(0);
        }
        _ => {}
    }
    check(THREAD_FLAGS & 0x1_0000 != 0, "CLONE_THREAD");
    identifiers();
    futexes();
    mutex();
    check(ping_pong(500) > 0, "ping-pong");
    print(b"ping-pong ok\n");
    registers();
    // The last thread's `exit` ends the cell with its status.
    syscall(nr::EXIT, [0; 6]);
    1
}

rhizo_cells::entry!(main);
