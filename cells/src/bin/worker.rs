//! One cell of the server workload (spec `benchmarks`, design D2 of
//! `add-reference-workloads`): `worker MODE PERIOD_MS SECS [PORT]`.
//!
//! It prints `ready` and sleeps: the host freezes it there as a template,
//! and each fork starts at once. From its first wake it runs one operation
//! every `PERIOD_MS` (the first after a random part of a period, so forks
//! spread out) for `SECS` seconds, then prints `ops=N` and exits with 0.
//! Any failed operation exits with 1. Modes:
//!
//! - `cpu`: 2,000 rounds of a xorshift generator;
//! - `file`: writes 4 KiB at the start of `work.bin` and reads them back;
//! - `pair`: listens on 127.0.0.1:`PORT` before `ready` (each fork listens
//!   again in its own loopback network, design D5 of `add-cell-hierarchy`)
//!   with a server thread that accepts one connection (then closes the
//!   listener) and echoes 64-byte
//!   messages until the other end closes it, without sleeping; the first
//!   thread connects to it and sends one 64-byte message per operation,
//!   checking its echo.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use rhizo_cells::{
    arg, check, join, now_ns, nr, number, print, read_full, sleep_ns, spawn_thread, syscall,
    tcp_connect, tcp_listen, tcp_socket, write_all,
};

const AT_FDCWD: usize = -100isize as usize;
const ECONNREFUSED: isize = -111;
const BLOCK: usize = 4096;

static mut BLOCK_BUF: [u8; BLOCK] = [0; BLOCK];

/// A random `u64` from the kernel.
fn random() -> u64 {
    let mut r = 0u64;
    syscall(nr::GETRANDOM, [&mut r as *mut u64 as usize, 8, 0, 0, 0, 0]);
    r
}

fn crunch(seed: u64) -> u64 {
    let mut x = seed | 1;
    for _ in 0..2_000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
    }
    x
}

fn file_op(fd: usize, round: u8) {
    // SAFETY: single-threaded program; the only reference to BLOCK_BUF.
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(BLOCK_BUF) };
    buf.fill(round);
    let n = syscall(nr::PWRITE64, [fd, buf.as_ptr() as usize, BLOCK, 0, 0, 0]);
    check(n == BLOCK as isize, "write");
    buf.fill(!round);
    let n = syscall(nr::PREAD64, [fd, buf.as_mut_ptr() as usize, BLOCK, 0, 0, 0]);
    check(
        n == BLOCK as isize && buf.iter().all(|&b| b == round),
        "read back",
    );
}

const STACK: usize = 16 * 1024;

#[repr(C, align(16))]
struct Stack([u8; STACK]);

static mut SERVER_STACK: Stack = Stack([0; STACK]);
static SERVER_TID: AtomicU32 = AtomicU32::new(0);
static LISTENER: AtomicUsize = AtomicUsize::new(0);

/// The server thread of `pair`: echoes on one connection until it closes.
extern "C" fn serve(_: usize) -> usize {
    let l = LISTENER.load(Ordering::Acquire);
    let s = syscall(nr::ACCEPT4, [l, 0, 0, 0, 0, 0]);
    check(s >= 0, "accept");
    // One connection only: the kernel's sockets stay at two per pair.
    syscall(nr::CLOSE, [l, 0, 0, 0, 0, 0]);
    let mut msg = [0u8; 64];
    loop {
        match read_full(s as usize, &mut msg) {
            0 => return 0,
            64 => check(write_all(s as usize, &msg) == 64, "echo"),
            _ => check(false, "receive"),
        }
    }
}

fn connect(port: u16) -> usize {
    loop {
        let c = tcp_socket(0);
        check(c >= 0, "socket");
        match tcp_connect(c as usize, port) {
            0 => return c as usize,
            ECONNREFUSED => {
                syscall(nr::CLOSE, [c as usize, 0, 0, 0, 0, 0]);
                sleep_ns(1_000_000);
            }
            _ => check(false, "connect"),
        }
    }
}

fn main() -> i32 {
    let mode = arg(1).unwrap_or(b"cpu");
    let period = arg(2).map_or(100, number) as u64 * 1_000_000;
    let secs = arg(3).map_or(1, number) as u64;
    let port = arg(4).map_or(0, number) as u16;
    if mode == b"pair" {
        let l = tcp_socket(0);
        check(l >= 0 && tcp_listen(l as usize, port) == 0, "listen");
        LISTENER.store(l as usize, Ordering::Release);
        // Only the address is taken: the stack is the server thread's alone.
        let top = core::ptr::addr_of_mut!(SERVER_STACK) as usize + STACK;
        check(spawn_thread(serve, 0, top, 0, &SERVER_TID) > 0, "clone");
    }
    print(b"ready\n");
    sleep_ns(3_600_000_000_000);
    let fd = (mode == b"file").then(|| {
        let flags = 0x2 | 0x40 | 0x200; // O_RDWR | O_CREAT | O_TRUNC
        let path = b"work.bin\0";
        let fd = syscall(
            nr::OPENAT,
            [AT_FDCWD, path.as_ptr() as usize, flags, 0o644, 0, 0],
        );
        check(fd >= 0, "open");
        fd as usize
    });
    let peer = (mode == b"pair").then(|| connect(port));
    let end = now_ns() + secs * 1_000_000_000;
    sleep_ns(random() % period.max(1));
    let (mut ops, mut seed) = (0u64, random());
    let mut msg = [0u8; 64];
    while now_ns() < end {
        match (fd, peer) {
            (Some(fd), _) => file_op(fd, ops as u8),
            (_, Some(c)) => {
                msg.fill(ops as u8);
                let mut back = [0u8; 64];
                check(write_all(c, &msg) == 64, "send");
                check(read_full(c, &mut back) == 64 && back == msg, "echo");
            }
            _ => seed = crunch(seed),
        }
        ops += 1;
        sleep_ns(period);
    }
    if let Some(c) = peer {
        syscall(nr::CLOSE, [c, 0, 0, 0, 0, 0]);
        join(&SERVER_TID);
    }
    // Keeps the computation from being optimized away.
    check(seed != 0, "crunch");
    report(ops)
}

/// Prints `ops=N` and exits with 0.
fn report(ops: u64) -> i32 {
    let mut line = *b"ops=00000000000000000000\n";
    let mut n = ops;
    for d in line[4..24].iter_mut().rev() {
        *d = b'0' + (n % 10) as u8;
        n /= 10;
    }
    print(&line);
    0
}

rhizo_cells::entry!(main);
