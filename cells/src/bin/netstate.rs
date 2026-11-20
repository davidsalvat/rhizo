//! What a cell finds open after a pause (specs `networking`, `snapshots`
//! and `fork`): it writes into a pipe, listens on 127.0.0.1:PORT (the first
//! argument, a free port by default) and connects to itself, prints `ready` and
//! sleeps 20 ms (where a host suspends, resumes or forks it). Then it
//! reports the pipe's bytes, whether its connection is still open or was
//! reset, and whether its listener still accepts a new connection.
#![no_std]
#![no_main]

use rhizo_cells::{
    arg, check, local_port, nr, number, print, sleep_ns, syscall, tcp_connect, tcp_listen,
    tcp_socket,
};

const SOCK_NONBLOCK: usize = 0x800;
const EAGAIN: isize = -11;
const ECONNRESET: isize = -104;

fn main() -> i32 {
    let port = arg(1).map_or(0, number) as u16;
    let mut fds = [0i32; 2];
    check(
        syscall(nr::PIPE2, [fds.as_mut_ptr() as usize, 0, 0, 0, 0, 0]) == 0,
        "pipe2",
    );
    check(
        syscall(
            nr::WRITE,
            [fds[1] as usize, b"kept".as_ptr() as usize, 4, 0, 0, 0],
        ) == 4,
        "write pipe",
    );
    let l = tcp_socket(0);
    check(l >= 0 && tcp_listen(l as usize, port) == 0, "listen");
    let port = local_port(l as usize);
    let c = tcp_socket(SOCK_NONBLOCK);
    check(c >= 0, "socket");
    let r = tcp_connect(c as usize, port);
    check(r == 0 || r == -115, "connect"); // or EINPROGRESS
    let s = syscall(nr::ACCEPT4, [l as usize, 0, 0, 0, 0, 0]);
    check(s >= 0, "accept");
    print(b"ready\n");
    sleep_ns(20_000_000);

    let mut buf = [0u8; 8];
    let n = syscall(
        nr::READ,
        [fds[0] as usize, buf.as_mut_ptr() as usize, 8, 0, 0, 0],
    );
    check(n == 4 && &buf[..4] == b"kept", "pipe");
    print(b"pipe kept\n");
    match syscall(
        nr::READ,
        [c as usize, buf.as_mut_ptr() as usize, 8, 0, 0, 0],
    ) {
        EAGAIN => print(b"connection open\n"),
        ECONNRESET => print(b"connection reset\n"),
        _ => check(false, "connection state"),
    }
    let again = tcp_socket(0);
    check(
        again >= 0 && tcp_connect(again as usize, port) == 0,
        "connect again",
    );
    let s = syscall(nr::ACCEPT4, [l as usize, 0, 0, 0, 0, 0]);
    check(s >= 0, "accept again");
    print(b"listener ok\n");
    0
}

rhizo_cells::entry!(main);
