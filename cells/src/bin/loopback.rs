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
//!   first echo (where a host suspends it), then `twice` after the second.
//!
//! Two cells, one serving and one pinging, measure `cell.loopback_rtt_us`;
//! one pinging the host's echo server through the network card measures
//! `net.tcp_rtt_us`; `twice`, suspended after `once`, measures
//! `cell.tcp_resume_us`.
#![no_std]
#![no_main]

use rhizo_cells::{
    arg, check, local_port, loopback, nr, number, print, read_full, sleep_ns, syscall, tcp_connect,
    tcp_listen, tcp_socket, write_all, yield_now,
};

const ECONNREFUSED: isize = -111;

fn serve(port: u16, n: usize) -> i32 {
    let l = tcp_socket(0);
    check(l >= 0 && tcp_listen(l as usize, port) == 0, "listen");
    let s = syscall(nr::ACCEPT4, [l as usize, 0, 0, 0, 0, 0]);
    check(s >= 0, "accept");
    let mut msg = [0u8; 64];
    for _ in 0..n {
        check(read_full(s as usize, &mut msg) == 64, "receive");
        check(write_all(s as usize, &msg) == 64, "echo");
    }
    print(b"served\n");
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
        _ => alone(),
    }
}

rhizo_cells::entry!(main);
