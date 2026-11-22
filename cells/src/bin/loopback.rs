//! TCP over the loopback network (spec `networking`).
//!
//! - no argument: both ends in this program: listen on a free port, connect,
//!   accept, exchange three messages, print `loopback ok`;
//! - `serve PORT N`: accept one connection on 127.0.0.1:PORT and echo N
//!   64-byte messages back, print `served`;
//! - `ping PORT N [ADDR]`: connect to ADDR:PORT (127.0.0.1 by default;
//!   retrying while nobody listens), send N 64-byte messages and check each
//!   echo, print `pinged`;
//! - `twice PORT`: as `ping PORT 2`, but print `once` and yield after the
//!   first echo (where a host suspends it), then `twice` after the second;
//! - `pair PORT N [wait|yield]`: both in one cell, as two threads (each cell has a
//!   loopback network of its own, design D5 of `add-cell-hierarchy`): a
//!   server thread does `serve PORT N` while the first thread does `ping
//!   PORT N` (after sleeping an hour with `wait`, until the host wakes it
//!   or a copy finds its sleep expired; with `yield`, yielding after each
//!   round trip, where a host may suspend it), then print `paired`;
//! - `pair PORT twice`: the same, the first thread doing `twice PORT`;
//! - `sibling PORT`: connect to 127.0.0.1:PORT once, expecting
//!   `ECONNREFUSED` (another cell's listener is not in this cell's
//!   network), then listen on PORT itself; print `refused, then bound`.
//!
//! A `pair` cell measures `cell.loopback_rtt_us`; one pinging the host's
//! echo server through the network card measures `net.tcp_rtt_us`; `pair
//! PORT twice`, suspended after `once`, measures `cell.tcp_resume_us`.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use rhizo_cells::{
    arg, check, join, local_port, loopback, nr, number, print, read_full, sleep_ns, spawn_thread,
    syscall, tcp_connect, tcp_listen, tcp_socket, write_all, yield_now,
};

const ECONNREFUSED: isize = -111;

/// A socket listening on 127.0.0.1:`port`.
fn listen(port: u16) -> usize {
    let l = tcp_socket(0);
    check(l >= 0 && tcp_listen(l as usize, port) == 0, "listen");
    l as usize
}

fn serve(port: u16, n: usize) -> i32 {
    echo(listen(port), n);
    print(b"served\n");
    0
}

/// Accepts one connection on `l` and echoes `n` 64-byte messages back.
fn echo(l: usize, n: usize) {
    let s = syscall(nr::ACCEPT4, [l, 0, 0, 0, 0, 0]);
    check(s >= 0, "accept");
    let mut msg = [0u8; 64];
    for _ in 0..n {
        check(read_full(s as usize, &mut msg) == 64, "receive");
        check(write_all(s as usize, &msg) == 64, "echo");
    }
}

const STACK: usize = 16 * 1024;

#[repr(C, align(16))]
struct Stack([u8; STACK]);

static mut SERVER_STACK: Stack = Stack([0; STACK]);
static SERVER_TID: AtomicU32 = AtomicU32::new(0);
/// The listener and message count the server thread works with.
static SERVER_LISTENER: AtomicUsize = AtomicUsize::new(0);
static SERVER_N: AtomicUsize = AtomicUsize::new(0);

extern "C" fn server_thread(_: usize) -> usize {
    echo(
        SERVER_LISTENER.load(Ordering::Acquire),
        SERVER_N.load(Ordering::Acquire),
    );
    0
}

/// Both ends in one cell: a server thread echoing `n` messages on `port`,
/// and this thread as the client (`twice`: as `twice PORT`).
fn pair(port: u16, n: usize, then: Option<&[u8]>, twice_mode: bool) -> i32 {
    SERVER_LISTENER.store(listen(port), Ordering::Release);
    SERVER_N.store(if twice_mode { 2 } else { n }, Ordering::Release);
    // Only the address is taken: the stack is the server thread's alone.
    let top = core::ptr::addr_of_mut!(SERVER_STACK) as usize + STACK;
    check(
        spawn_thread(server_thread, 0, top, 0, &SERVER_TID) > 0,
        "clone",
    );
    if then == Some(b"wait") {
        sleep_ns(3_600_000_000_000);
    }
    if twice_mode {
        twice(port);
    } else {
        let c = connect(port, [127, 0, 0, 1]);
        for i in 0..n {
            round_trip(c, i);
            if then == Some(b"yield") {
                yield_now();
            }
        }
    }
    join(&SERVER_TID);
    print(b"paired\n");
    0
}

/// Another cell's port is not in this cell's network: connecting is
/// refused, and the port is free here.
fn sibling(port: u16) -> i32 {
    let c = tcp_socket(0);
    check(c >= 0, "socket");
    let addr = loopback(port);
    let r = syscall(
        nr::CONNECT,
        [c as usize, addr.as_ptr() as usize, 16, 0, 0, 0],
    );
    check(r == ECONNREFUSED, "connection refused");
    listen(port);
    print(b"refused, then bound\n");
    0
}

/// A dotted IPv4 address (127.0.0.1 if malformed).
fn ipv4(text: &[u8]) -> [u8; 4] {
    let mut out = [0u8; 4];
    let mut parts = text.split(|&b| b == b'.');
    for o in &mut out {
        match parts.next() {
            Some(p) if !p.is_empty() => *o = number(p) as u8,
            _ => return [127, 0, 0, 1],
        }
    }
    out
}

/// A connected TCP socket to `ip:port`.
fn connect(port: u16, ip: [u8; 4]) -> usize {
    let c = tcp_socket(0);
    check(c >= 0, "socket");
    let mut addr = loopback(port);
    addr[4..8].copy_from_slice(&ip);
    let mut tries = 0;
    loop {
        match syscall(
            nr::CONNECT,
            [c as usize, addr.as_ptr() as usize, 16, 0, 0, 0],
        ) {
            0 => break,
            // A local server may not listen yet; a remote one is retried
            // only a few times (each refusal may take a timeout).
            ECONNREFUSED if tries < if ip[0] == 127 { 10_000 } else { 20 } => {
                tries += 1;
                sleep_ns(100_000);
            }
            _ => check(false, "connect"),
        }
    }
    c as usize
}

/// Sends a 64-byte message of `i` on `c` and checks its echo.
fn round_trip(c: usize, i: usize) {
    let (mut msg, mut back) = ([0u8; 64], [0u8; 64]);
    msg.fill(i as u8);
    check(write_all(c, &msg) == 64, "send");
    check(read_full(c, &mut back) == 64 && back == msg, "echo");
}

fn ping(port: u16, n: usize, ip: [u8; 4]) -> i32 {
    let c = connect(port, ip);
    for i in 0..n {
        round_trip(c, i);
    }
    print(b"pinged\n");
    0
}

fn twice(port: u16) -> i32 {
    let c = connect(port, [127, 0, 0, 1]);
    round_trip(c, 1);
    print(b"once\n");
    yield_now();
    round_trip(c, 2);
    print(b"twice\n");
    0
}

/// Both ends in one program, one message at a time.
fn alone() -> i32 {
    let l = tcp_socket(0);
    check(l >= 0 && tcp_listen(l as usize, 0) == 0, "listen");
    let port = local_port(l as usize);
    let c = tcp_socket(0);
    check(c >= 0 && tcp_connect(c as usize, port) == 0, "connect");
    let s = syscall(nr::ACCEPT4, [l as usize, 0, 0, 0, 0, 0]);
    check(s >= 0, "accept");
    let mut buf = [0u8; 5];
    for word in [b"one  ", b"two  ", b"three"] {
        check(write_all(c as usize, word) == 5, "send");
        check(
            read_full(s as usize, &mut buf) == 5 && &buf == word,
            "receive",
        );
        check(write_all(s as usize, &buf) == 5, "echo");
        check(
            read_full(c as usize, &mut buf) == 5 && &buf == word,
            "echo back",
        );
    }
    print(b"loopback ok\n");
    0
}

fn main() -> i32 {
    let port = arg(2).map_or(0, number) as u16;
    let n = arg(3).map_or(1, number);
    match arg(1) {
        Some(b"serve") => serve(port, n),
        Some(b"ping") => ping(port, n, arg(4).map_or([127, 0, 0, 1], ipv4)),
        Some(b"twice") => twice(port),
        Some(b"pair") if arg(3) == Some(b"twice") => pair(port, 2, None, true),
        Some(b"pair") => pair(port, n, arg(4), false),
        Some(b"sibling") => sibling(port),
        _ => alone(),
    }
}

rhizo_cells::entry!(main);
