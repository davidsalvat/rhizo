//! What a cell's egress policy lets it reach outside (design D6 and D7 of
//! `add-cell-hierarchy`).
//!
//! - `ADDR:PORT...`: connects to each destination with TCP and prints one
//!   letter for it: `C` connected, `D` denied (`EACCES`), `R` any other
//!   failure (refused, unreachable); then a newline;
//! - `take N ADDR:PORT`: starts N connections to ADDR:PORT through the
//!   network card and binds N sockets to ephemeral ports, closing each,
//!   prints `holding` and sleeps until the host wakes it (a neighbour that
//!   uses card and loopback ports while another cell watches its own).
//!
//! Never run on Linux.
#![no_std]
#![no_main]

use rhizo_cells::{arg, check, loopback, nr, number, print, sleep_ns, syscall, tcp_socket};

const SOCK_NONBLOCK: usize = 0x800;
const EACCES: isize = -13;
const EINPROGRESS: isize = -115;

/// A `sockaddr_in` for `ADDR:PORT`, as written in `text`.
fn address(text: &[u8]) -> [u8; 16] {
    let mut parts = text.splitn(2, |&b| b == b':');
    let (ip, port) = (parts.next().unwrap_or_default(), parts.next());
    let mut addr = loopback(port.map_or(0, number) as u16);
    for (o, part) in addr[4..8].iter_mut().zip(ip.split(|&b| b == b'.')) {
        *o = number(part) as u8;
    }
    addr
}

fn close(fd: usize) {
    syscall(nr::CLOSE, [fd, 0, 0, 0, 0, 0]);
}

/// Connects to `addr` and says how it went.
fn try_connect(addr: &[u8; 16]) -> u8 {
    let s = tcp_socket(0);
    check(s >= 0, "socket");
    let r = syscall(
        nr::CONNECT,
        [s as usize, addr.as_ptr() as usize, 16, 0, 0, 0],
    );
    close(s as usize);
    match r {
        0 => b'C',
        EACCES => b'D',
        _ => b'R',
    }
}

fn take(n: usize, addr: &[u8; 16]) {
    for _ in 0..n {
        let s = tcp_socket(SOCK_NONBLOCK);
        check(s >= 0, "socket");
        let r = syscall(
            nr::CONNECT,
            [s as usize, addr.as_ptr() as usize, 16, 0, 0, 0],
        );
        check(r == EINPROGRESS || r == 0, "connect through the card");
        close(s as usize);
        let b = tcp_socket(0);
        check(b >= 0, "socket");
        let any = loopback(0);
        let r = syscall(nr::BIND, [b as usize, any.as_ptr() as usize, 16, 0, 0, 0]);
        check(r == 0, "bind");
        close(b as usize);
    }
    print(b"holding\n");
    sleep_ns(3_600_000_000_000);
}

fn main() -> i32 {
    if arg(1) == Some(b"take") {
        take(
            arg(2).map_or(0, number),
            &address(arg(3).unwrap_or_default()),
        );
        return 0;
    }
    let mut said = [0u8; 16];
    let mut n = 0;
    while let Some(target) = arg(n + 1) {
        check(n < said.len() - 1, "at most 15 destinations");
        said[n] = try_connect(&address(target));
        n += 1;
    }
    said[n] = b'\n';
    print(&said[..=n]);
    0
}

rhizo_cells::entry!(main);
