//! Cell handles (task 2.5 of `add-cell-hierarchy`, design D9 and D10):
//! the four ways a cell uses a handle the host gave it on another cell,
//! through `/run/rhizo/cell/<label>`.
//!
//! - `serve`: opens its `/run/rhizo/inbox`, accepts one channel and prints
//!   `peer P` (the label `getpeername` names), reads one message and prints
//!   `got M`, answers `pong`, then reads to end of file and prints
//!   `closed`;
//! - `client LABEL`: connects a `SOCK_SEQPACKET` socket to its handle
//!   `LABEL` (the message right), sends `ping` and prints `reply M`;
//! - `observe LABEL`: opens its handle for reading (the observe right),
//!   prints `observing`, then copies every message it reads to its output
//!   as it came (frames of the control protocol) until end of file, and
//!   prints `end`;
//! - `input LABEL TEXT`: opens its handle for writing (the input right),
//!   writes `TEXT` and a newline, and prints `sent`;
//! - `echo`: reads one line from its standard input and prints `echo
//!   LINE`;
//! - `refused LABEL`: tries the three uses of its handle `LABEL` and prints
//!   `connect E observe E input E`, each the error number (0 for none).
//!
//! Never run on Linux.
#![no_std]
#![no_main]

use rhizo_cells::{arg, check, nr, print, syscall};

#[cfg(target_arch = "aarch64")]
const GETPEERNAME: usize = 205;
#[cfg(target_arch = "x86_64")]
const GETPEERNAME: usize = 52;

const AT_FDCWD: usize = -100isize as usize;
const AF_UNIX: usize = 1;
const SOCK_SEQPACKET: usize = 5;
const O_WRONLY: usize = 1;
const O_RDWR: usize = 2;

/// `/run/rhizo/<rest>` and a NUL, in `buf`.
fn rhizo_path<'a>(buf: &'a mut [u8; 128], rest: &[&[u8]]) -> &'a [u8] {
    let mut at = 0;
    for part in [&b"/run/rhizo/"[..]].iter().chain(rest) {
        buf[at..at + part.len()].copy_from_slice(part);
        at += part.len();
    }
    buf[at] = 0;
    &buf[..=at]
}

fn open(path: &[u8], flags: usize) -> isize {
    syscall(
        nr::OPENAT,
        [AT_FDCWD, path.as_ptr() as usize, flags, 0, 0, 0],
    )
}

fn read(fd: usize, buf: &mut [u8]) -> isize {
    syscall(
        nr::READ,
        [fd, buf.as_mut_ptr() as usize, buf.len(), 0, 0, 0],
    )
}

fn write(fd: usize, bytes: &[u8]) -> isize {
    syscall(
        nr::WRITE,
        [fd, bytes.as_ptr() as usize, bytes.len(), 0, 0, 0],
    )
}

/// Prints `what`, then `n` in decimal (with its sign) and a separator.
fn print_number(what: &[u8], n: isize, end: &[u8]) {
    print(what);
    let mut digits = [0u8; 21];
    let mut at = digits.len();
    let mut v = n.unsigned_abs();
    loop {
        at -= 1;
        digits[at] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    if n < 0 {
        at -= 1;
        digits[at] = b'-';
    }
    print(&digits[at..]);
    print(end);
}

/// A sequenced-packet socket connected to handle `label`, or the error.
fn connect(label: &[u8]) -> isize {
    let s = syscall(nr::SOCKET, [AF_UNIX, SOCK_SEQPACKET, 0, 0, 0, 0]);
    check(s >= 0, "socket");
    let mut addr = [0u8; 110];
    addr[0] = AF_UNIX as u8;
    let mut path = [0u8; 128];
    let p = rhizo_path(&mut path, &[b"cell/", label]);
    addr[2..2 + p.len()].copy_from_slice(p);
    let r = syscall(
        nr::CONNECT,
        [s as usize, addr.as_ptr() as usize, 2 + p.len(), 0, 0, 0],
    );
    if r < 0 { r } else { s }
}

/// Handle `label` opened with `flags`.
fn open_handle(label: &[u8], flags: usize) -> isize {
    let mut path = [0u8; 128];
    open(rhizo_path(&mut path, &[b"cell/", label]), flags)
}

fn serve() {
    let mut path = [0u8; 128];
    let inbox = open(rhizo_path(&mut path, &[b"inbox"]), O_RDWR);
    check(inbox >= 0, "open the inbox");
    let ch = syscall(nr::ACCEPT4, [inbox as usize, 0, 0, 0, 0, 0]);
    check(ch >= 0, "accept a channel");
    let mut addr = [0u8; 112];
    let mut len: u32 = 110;
    let r = syscall(
        GETPEERNAME,
        [
            ch as usize,
            addr.as_mut_ptr() as usize,
            &mut len as *mut u32 as usize,
            0,
            0,
            0,
        ],
    );
    check(r == 0 && len > 2, "getpeername");
    print(b"peer ");
    print(&addr[2..len as usize - 1]);
    print(b"\n");
    let mut buf = [0u8; 64];
    let n = read(ch as usize, &mut buf);
    check(n > 0, "a message");
    print(b"got ");
    print(&buf[..n as usize]);
    print(b"\n");
    check(write(ch as usize, b"pong") == 4, "answer");
    check(read(ch as usize, &mut buf) == 0, "end of file");
    print(b"closed\n");
}

fn client(label: &[u8]) {
    let s = connect(label);
    if s < 0 {
        print_number(b"connect ", s, b"\n");
        return;
    }
    check(write(s as usize, b"ping") == 4, "send");
    let mut buf = [0u8; 64];
    let n = read(s as usize, &mut buf);
    check(n > 0, "a reply");
    print(b"reply ");
    print(&buf[..n as usize]);
    print(b"\n");
}

fn observe(label: &[u8]) {
    let fd = open_handle(label, 0);
    if fd < 0 {
        print_number(b"observe ", fd, b"\n");
        return;
    }
    print(b"observing\n");
    let mut buf = [0u8; 1024];
    loop {
        let n = read(fd as usize, &mut buf);
        check(n >= 0, "read the stream");
        if n == 0 {
            break;
        }
        print(&buf[..n as usize]);
    }
    print(b"end\n");
}

fn input(label: &[u8], text: &[u8]) {
    let fd = open_handle(label, O_WRONLY);
    if fd < 0 {
        print_number(b"input ", fd, b"\n");
        return;
    }
    check(write(fd as usize, text) == text.len() as isize, "write");
    check(write(fd as usize, b"\n") == 1, "write");
    print(b"sent\n");
}

fn echo() {
    let mut buf = [0u8; 256];
    let n = read(0, &mut buf);
    check(n > 0, "a line");
    print(b"echo ");
    print(&buf[..n as usize]);
}

fn refused(label: &[u8]) {
    let c = connect(label);
    let o = open_handle(label, 0);
    let i = open_handle(label, O_WRONLY);
    let err = |r: isize| if r < 0 { r } else { 0 };
    print_number(b"connect ", err(c), b" ");
    print_number(b"observe ", err(o), b" ");
    print_number(b"input ", err(i), b"\n");
}

fn main() -> i32 {
    let label = arg(2).unwrap_or(b"");
    match arg(1) {
        Some(b"serve") => serve(),
        Some(b"client") => client(label),
        Some(b"observe") => observe(label),
        Some(b"input") => input(label, arg(3).unwrap_or(b"")),
        Some(b"echo") => echo(),
        Some(b"refused") => refused(label),
        _ => return 2,
    }
    0
}

rhizo_cells::entry!(main);
