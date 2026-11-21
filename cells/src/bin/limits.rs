//! Prints `ready` and yields: the host sets its quotas, forks, freezes or
//! suspends it there. Then makes Unix sockets until refused, closes them,
//! opens `.` until refused, and prints what it got and why it stopped:
//! `nofile L sockets N errno E files M errno F` (spec `isolation`,
//! quotas). Without a quota set by the host: 256 sockets (the default), and
//! 1021 files.
#![no_std]
#![no_main]

use rhizo_cells::{nr, print, syscall, yield_now};

/// Counts how many times `make` gives a descriptor, closing them all after;
/// returns the count and the errno that stopped it.
fn until_refused(make: impl Fn() -> isize) -> (usize, usize) {
    let mut fds = [0usize; 1024];
    let mut n = 0;
    let errno = loop {
        let fd = make();
        if fd < 0 || n == fds.len() {
            break (-fd) as usize;
        }
        fds[n] = fd as usize;
        n += 1;
    };
    for &fd in &fds[..n] {
        syscall(nr::CLOSE, [fd, 0, 0, 0, 0, 0]);
    }
    (n, errno)
}

fn decimal(out: &mut [u8], at: &mut usize, mut v: usize) {
    let mut digits = [0u8; 20];
    let mut i = digits.len();
    loop {
        i -= 1;
        digits[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    for &d in &digits[i..] {
        out[*at] = d;
        *at += 1;
    }
}

fn main() -> i32 {
    print(b"ready\n");
    yield_now();
    let mut rlimit = [0u64; 2];
    #[cfg(target_arch = "x86_64")]
    const GETRLIMIT: usize = 97;
    #[cfg(target_arch = "aarch64")]
    const GETRLIMIT: usize = 163;
    syscall(GETRLIMIT, [7, rlimit.as_mut_ptr() as usize, 0, 0, 0, 0]);
    let sockets = until_refused(|| syscall(nr::SOCKET, [1, 1, 0, 0, 0, 0]));
    let files = until_refused(|| {
        syscall(
            nr::OPENAT,
            [-100isize as usize, c".".as_ptr() as usize, 0, 0, 0, 0],
        )
    });
    let mut line = [0u8; 128];
    let mut at = 0;
    for (text, value) in [
        (&b"nofile "[..], rlimit[0] as usize),
        (b" sockets ", sockets.0),
        (b" errno ", sockets.1),
        (b" files ", files.0),
        (b" errno ", files.1),
    ] {
        line[at..at + text.len()].copy_from_slice(text);
        at += text.len();
        decimal(&mut line, &mut at, value);
    }
    line[at] = b'\n';
    print(&line[..=at]);
    0
}

rhizo_cells::entry!(main);
