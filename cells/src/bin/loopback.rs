//! TCP over the loopback network (spec `networking`).
//!
//! - no argument: both ends in this program: listen on a free port, connect,
//!   accept, exchange three messages, print `loopback ok`;
//! - `serve PORT N`: accept one connection on 127.0.0.1:PORT and echo N
//!   64-byte messages back, print `served`;
//! - `ping PORT N`: connect to 127.0.0.1:PORT (retrying while nobody
//!   listens), send N 64-byte messages and check each echo, print `pinged`.
//!
//! Two cells, one serving and one pinging, measure `cell.loopback_rtt_us`.
#![no_std]
#![no_main]

use rhizo_cells::{
    arg, check, local_port, nr, number, print, read_full, sleep_ns, syscall, tcp_connect,
    tcp_listen, tcp_socket, write_all,
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

fn ping(port: u16, n: usize) -> i32 {
    let c = tcp_socket(0);
    check(c >= 0, "socket");
    let mut tries = 0;
    loop {
        match tcp_connect(c as usize, port) {
            0 => break,
            ECONNREFUSED if tries < 10_000 => {
                tries += 1;
                sleep_ns(100_000);
            }
            _ => check(false, "connect"),
        }
    }
    let (mut msg, mut back) = ([0u8; 64], [0u8; 64]);
    for i in 0..n {
        msg.fill(i as u8);
        check(write_all(c as usize, &msg) == 64, "send");
        check(
            read_full(c as usize, &mut back) == 64 && back == msg,
            "echo",
        );
    }
    print(b"pinged\n");
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
        Some(b"ping") => ping(port, n),
        _ => alone(),
    }
}

rhizo_cells::entry!(main);
