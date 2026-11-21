//! Hostile pair (change `close-cross-cell-leaks`, findings S24 to S26): an
//! observer looks at what the kernel tells it while a neighbour cell works,
//! and prints what it sees; nothing of the neighbour may show.
//!
//! - `ports PORT`: binds a socket to an ephemeral port, prints `ready`,
//!   waits for the neighbour's connection on 127.0.0.1:PORT and its end,
//!   binds another socket and prints `ports +N`, N the distance between the
//!   two ports (1 when only its own binds move its ports);
//! - `memory PORT`: reads `sysinfo`, prints `ready`, reads it again once
//!   the neighbour connected (while the neighbour holds its memory), and
//!   prints `memory total=T change=C procs=P`: the total in MiB, how much
//!   the free memory moved in MiB, and the processes;
//! - `neighbour PORT MIB N`: takes and touches MIB MiB, binds N sockets to
//!   ephemeral ports and closes them, connects to 127.0.0.1:PORT and waits
//!   for the observer to close the connection;
//! - `page`: maps a page of a file of its own, prints `mapped`, sleeps
//!   until the host wakes it, reads the page again and prints `page ok`;
//! - `ids`: prints `pid P tid T` from `getpid` and `gettid`, yields three
//!   times (the host may fork or suspend it there), and prints them again
//!   (change `add-cell-hierarchy`, identifiers per domain).
//!
//! Never run on Linux.
#![no_std]
#![no_main]

use rhizo_cells::{
    arg, check, local_port, loopback, nr, number, print, sleep_ns, syscall, tcp_connect,
    tcp_listen, tcp_socket, yield_now,
};

const ECONNREFUSED: isize = -111;
const AT_FDCWD: usize = -100isize as usize;
const O_RDWR_CREAT: usize = 0o102;
const SHUT_WR: usize = 1;

fn print_number(mut n: u64) {
    let mut digits = [0u8; 20];
    let mut at = digits.len();
    loop {
        at -= 1;
        digits[at] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    print(&digits[at..]);
}

/// A TCP socket bound to an ephemeral loopback port, and that port.
fn ephemeral() -> (usize, u16) {
    let s = tcp_socket(0);
    check(s >= 0, "socket");
    let addr = loopback(0);
    let r = syscall(nr::BIND, [s as usize, addr.as_ptr() as usize, 16, 0, 0, 0]);
    check(r == 0, "bind");
    (s as usize, local_port(s as usize))
}

/// Reads `fd` until the peer closes it.
fn read_to_end(fd: usize) {
    let mut buf = [0u8; 64];
    while syscall(
        nr::READ,
        [fd, buf.as_mut_ptr() as usize, buf.len(), 0, 0, 0],
    ) > 0
    {}
}

/// Listens on `port`, prints `ready`, and accepts the neighbour.
fn wait_for_neighbour(port: u16) -> usize {
    let l = tcp_socket(0);
    check(l >= 0 && tcp_listen(l as usize, port) == 0, "listen");
    print(b"ready\n");
    let c = syscall(nr::ACCEPT4, [l as usize, 0, 0, 0, 0, 0]);
    check(c >= 0, "accept");
    c as usize
}

/// Lets the neighbour go and waits until it has closed its end.
fn let_go(c: usize) {
    check(
        syscall(nr::SHUTDOWN, [c, SHUT_WR, 0, 0, 0, 0]) == 0,
        "shutdown",
    );
    read_to_end(c);
}

fn ports(port: u16) {
    let (_, first) = ephemeral();
    let c = wait_for_neighbour(port);
    let_go(c);
    let (_, second) = ephemeral();
    let span = (60999 - 32768 + 1) as u32;
    let distance = (u32::from(second) + span - u32::from(first)) % span;
    print(b"ports +");
    print_number(distance.into());
    print(b"\n");
}

/// Total RAM, free RAM (bytes) and processes, as `sysinfo` gives them.
fn sysinfo() -> (u64, u64, u16) {
    let mut info = [0u8; 112];
    check(
        syscall(nr::SYSINFO, [info.as_mut_ptr() as usize, 0, 0, 0, 0, 0]) == 0,
        "sysinfo",
    );
    let word = |at: usize| u64::from_le_bytes(info[at..at + 8].try_into().unwrap());
    let unit = u64::from(u32::from_le_bytes(info[104..108].try_into().unwrap()));
    let procs = u16::from_le_bytes([info[80], info[81]]);
    (word(32) * unit, word(40) * unit, procs)
}

fn memory(port: u16) {
    let (total, before, _) = sysinfo();
    let c = wait_for_neighbour(port);
    let (_, after, procs) = sysinfo();
    let_go(c);
    print(b"memory total=");
    print_number(total >> 20);
    print(b" change=");
    print_number(before.abs_diff(after) >> 20);
    print(b" procs=");
    print_number(procs.into());
    print(b"\n");
}

fn neighbour(port: u16, mib: usize, n: usize) {
    if mib > 0 {
        let size = mib << 20;
        let p = syscall(nr::MMAP, [0, size, 3, 0x22, usize::MAX, 0]);
        check(p > 0, "mmap");
        for page in (0..size).step_by(4096) {
            // SAFETY: inside the mapping just made.
            unsafe { ((p as usize + page) as *mut u8).write_volatile(1) };
        }
    }
    for _ in 0..n {
        let (s, _) = ephemeral();
        syscall(nr::CLOSE, [s, 0, 0, 0, 0, 0]);
    }
    let c = tcp_socket(0);
    check(c >= 0, "socket");
    loop {
        match tcp_connect(c as usize, port) {
            0 => break,
            ECONNREFUSED => yield_now(),
            _ => check(false, "connect"),
        }
    }
    read_to_end(c as usize);
}

fn page() {
    let path = c"/page".as_ptr() as usize;
    let fd = syscall(nr::OPENAT, [AT_FDCWD, path, O_RDWR_CREAT, 0o600, 0, 0]);
    check(fd >= 0, "open");
    let bytes = [b'p'; 4096];
    let written = syscall(
        nr::PWRITE64,
        [fd as usize, bytes.as_ptr() as usize, bytes.len(), 0, 0, 0],
    );
    check(written == 4096, "write");
    let at = syscall(nr::MMAP, [0, 4096, 1, 2, fd as usize, 0]);
    check(at > 0, "mmap");
    // SAFETY: a readable page just mapped.
    check(
        unsafe { (at as *const u8).read_volatile() } == b'p',
        "the page",
    );
    print(b"mapped\n");
    sleep_ns(1000 * 1_000_000_000);
    // SAFETY: as above, still mapped.
    check(
        unsafe { (at as *const u8).read_volatile() } == b'p',
        "the page",
    );
    print(b"page ok\n");
}

fn ids() {
    for round in 0..2 {
        print(b"pid ");
        print_number(syscall(nr::GETPID, [0; 6]) as u64);
        print(b" tid ");
        print_number(syscall(nr::GETTID, [0; 6]) as u64);
        print(b"\n");
        for _ in 0..3 * (1 - round) {
            yield_now();
        }
    }
}

fn main() -> i32 {
    let port = arg(2).map_or(0, number) as u16;
    match arg(1) {
        Some(b"ports") => ports(port),
        Some(b"memory") => memory(port),
        Some(b"neighbour") => neighbour(port, arg(3).map_or(0, number), arg(4).map_or(0, number)),
        Some(b"page") => page(),
        Some(b"ids") => ids(),
        _ => check(false, "mode"),
    }
    0
}

rhizo_cells::entry!(main);
