//! Hostile pair (change `close-cross-cell-leaks`, findings S24 to S26): an
//! observer looks at what the kernel tells it while a neighbour cell works,
//! and prints what it sees; nothing of the neighbour may show.
//!
//! The host runs the two in turn: cells share no loopback network to meet
//! on (design D5 of `add-cell-hierarchy`), and each sleeps until the host
//! wakes it.
//!
//! - `ports`: binds a socket to an ephemeral port, prints `ready` and
//!   sleeps while the neighbour works, binds another socket and prints
//!   `ports +N`, N the distance between the two ports (1 when only its own
//!   binds move its ports);
//! - `memory`: reads `sysinfo`, prints `ready` and sleeps, reads it again
//!   (while the neighbour holds its memory), and prints `memory total=T
//!   change=C procs=P`: the total in MiB, how much the free memory moved in
//!   MiB, and the processes;
//! - `neighbour MIB N`: takes and touches MIB MiB, binds N sockets to
//!   ephemeral ports and closes them, prints `holding` and sleeps, holding
//!   its memory;
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
    arg, check, local_port, loopback, nr, number, print, sleep_ns, syscall, tcp_socket, yield_now,
};

const AT_FDCWD: usize = -100isize as usize;
const O_RDWR_CREAT: usize = 0o102;

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

/// Prints `said` and sleeps until the host wakes it.
fn wait_for_host(said: &[u8]) {
    print(said);
    sleep_ns(3_600_000_000_000);
}

fn ports() {
    let (_, first) = ephemeral();
    wait_for_host(b"ready\n");
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

fn memory() {
    let (total, before, _) = sysinfo();
    wait_for_host(b"ready\n");
    let (_, after, procs) = sysinfo();
    print(b"memory total=");
    print_number(total >> 20);
    print(b" change=");
    print_number(before.abs_diff(after) >> 20);
    print(b" procs=");
    print_number(procs.into());
    print(b"\n");
}

fn neighbour(mib: usize, n: usize) {
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
    wait_for_host(b"holding\n");
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
    match arg(1) {
        Some(b"ports") => ports(),
        Some(b"memory") => memory(),
        Some(b"neighbour") => neighbour(arg(2).map_or(0, number), arg(3).map_or(0, number)),
        Some(b"page") => page(),
        Some(b"ids") => ids(),
        _ => check(false, "mode"),
    }
    0
}

rhizo_cells::entry!(main);
