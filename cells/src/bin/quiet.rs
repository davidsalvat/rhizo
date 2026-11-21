//! A connection that stays silent (change `fix-tcp-long-wait`):
//! `quiet PORT MS ADDR` connects to the echo server at ADDR:PORT, checks
//! one 64-byte echo, stays silent for MS milliseconds and checks another;
//! then closes, connects again and checks an echo, all within a second;
//! prints `quiet ok`.
//!
//! Its own program, not a mode of `loopback`: `loopback twice` is resumed
//! lazily by a benchmark, which counts the pages its code touches.
#![no_std]
#![no_main]

use rhizo_cells::{
    arg, check, loopback, now_ns, nr, number, print, read_full, sleep_ns, syscall, tcp_socket,
    write_all,
};

/// A dotted IPv4 address.
fn ipv4(text: &[u8]) -> [u8; 4] {
    let mut out = [0u8; 4];
    let mut parts = text.split(|&b| b == b'.');
    for o in &mut out {
        *o = parts.next().map_or(0, number) as u8;
    }
    out
}

/// A connected TCP socket to `ip:port`.
fn connect(port: u16, ip: [u8; 4]) -> usize {
    let c = tcp_socket(0);
    check(c >= 0, "socket");
    let mut addr = loopback(port);
    addr[4..8].copy_from_slice(&ip);
    let r = syscall(
        nr::CONNECT,
        [c as usize, addr.as_ptr() as usize, 16, 0, 0, 0],
    );
    check(r == 0, "connect");
    c as usize
}

/// Sends a 64-byte message of `i` on `c` and checks its echo.
fn round_trip(c: usize, i: u8) {
    let (msg, mut back) = ([i; 64], [0u8; 64]);
    check(write_all(c, &msg) == 64, "send");
    check(read_full(c, &mut back) == 64 && back == msg, "echo");
}

fn main() -> i32 {
    let port = arg(1).map_or(0, number) as u16;
    let ms = arg(2).map_or(0, number) as u64;
    let ip = ipv4(arg(3).unwrap_or(b"127.0.0.1"));
    let c = connect(port, ip);
    round_trip(c, 1);
    sleep_ns(ms * 1_000_000);
    round_trip(c, 2);
    check(syscall(nr::CLOSE, [c, 0, 0, 0, 0, 0]) == 0, "close");
    let t0 = now_ns();
    let c = connect(port, ip);
    round_trip(c, 3);
    check(now_ns() - t0 < 1_000_000_000, "reconnect within a second");
    print(b"quiet ok\n");
    0
}

rhizo_cells::entry!(main);
