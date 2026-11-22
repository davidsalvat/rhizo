//! Hostile colluders (design D0 and D12 of `add-cell-hierarchy`): cells of
//! two domains that try to signal each other through the kernel.
//!
//! - `probe [ADDR:PORT]`: the receiver. Prints `ready` and sleeps until the
//!   host wakes it, then runs a fixed probe and prints what the kernel told
//!   it, apart from times: identifiers, `sysinfo`, threads, memory taken
//!   until refused, a file written until refused, sockets made until
//!   refused, loopback binds and connections, ephemeral ports (as
//!   distances), a round trip with the echo server at ADDR:PORT through the
//!   network card, and pages of a file mapped; all of it twice. Nothing it
//!   prints may change when another domain works beside it (spec
//!   `isolation`, "No storage channel between domains");
//! - `send [ADDR:PORT]`: the sender. Does all of that as hard as it can,
//!   round after round, holding what it took for a few yields before giving
//!   it back: memory and a file to its limit, every socket it may hold
//!   (connections to ADDR:PORT first), the receiver's loopback ports, a
//!   hundred ephemeral ports, threads. Prints `sending` after its first
//!   round, then a dot per round, and never ends;
//! - `listen PORT`: listens on TCP port PORT of every address, binds UDP
//!   port PORT and a Unix socket `/colluder`, prints `listening` and sleeps
//!   until the host wakes it; then prints `heard nothing` if no connection,
//!   datagram or Unix connection arrived, `heard` otherwise;
//! - `knock PORT [ADDR]`: tries to reach a `listen` cell of another domain:
//!   TCP and UDP to 127.0.0.1:PORT, the Unix socket `/colluder`, and TCP and
//!   UDP to ADDR:PORT (the network card's own address) when given; prints a
//!   letter for each (`R` refused, `N` no such name, `D` denied, `S` sent,
//!   `W` still connecting, `C` connected, `?` anything else), then binds
//!   PORT itself and prints `bound`;
//! - `dial ADDR:PORT`: prints `ready` and sleeps until the host wakes it
//!   (where a host suspends it), then connects to ADDR:PORT and prints a
//!   letter as `knock` does (a loaded snapshot's reach);
//! - `bits START PERIOD N` and `count START PERIOD N`: a CPU-contention
//!   channel (`covert.cpu_bps`). From monotonic time START (ns), for N
//!   periods of PERIOD ns, the sender spins through a period for a 1 bit
//!   and sleeps through it for a 0 (bits from a fixed generator); the
//!   receiver counts the loops it makes in each period, takes a period
//!   below the median of the nine around it for a 1, and prints `bits N
//!   errors E`.
//!
//! Never run on Linux.
#![no_std]
#![no_main]

use core::sync::atomic::AtomicU32;
use rhizo_cells::{
    arg, check, join, local_port, loopback, now_ns, nr, number, print, read_full, sleep_ns,
    spawn_thread, syscall, tcp_listen, tcp_socket, write_all, yield_now,
};

#[cfg(target_arch = "aarch64")]
mod more {
    pub const UNLINKAT: usize = 35;
    pub const RECVFROM: usize = 207;
    pub const CLOCK_NANOSLEEP: usize = 115;
}
#[cfg(target_arch = "x86_64")]
mod more {
    pub const UNLINKAT: usize = 263;
    pub const RECVFROM: usize = 45;
    pub const CLOCK_NANOSLEEP: usize = 230;
}

const AT_FDCWD: usize = -100isize as usize;
const O_RDWR_CREAT: usize = 0o102;
const SOCK_DGRAM: usize = 2;
const SOCK_NONBLOCK: usize = 0x800;
const MSG_DONTWAIT: usize = 0x40;
const ENOENT: isize = -2;
const EAGAIN: isize = -11;
const EACCES: isize = -13;
const ECONNREFUSED: isize = -111;
const EALREADY: isize = -114;
const EINPROGRESS: isize = -115;
const MIB: usize = 1 << 20;
/// Loopback ports both colluders use.
const PORTS: [u16; 3] = [7400, 7401, 7402];
/// Most memory steps a cell takes (its limit is far below).
const STEPS: usize = 256;
/// Most sockets a cell holds (its limit is below).
const SOCKETS: usize = 64;

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

/// Prints ` key=n`.
fn field(key: &[u8], n: u64) {
    print(b" ");
    print(key);
    print(b"=");
    print_number(n);
}

/// Prints ` key=e` for a negated `errno` (or 0).
fn error(key: &[u8], r: isize) {
    field(key, r.min(0).unsigned_abs() as u64);
}

fn close(fd: usize) {
    syscall(nr::CLOSE, [fd, 0, 0, 0, 0, 0]);
}

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

/// `addr` with port `port`.
fn at_port(mut addr: [u8; 16], port: u16) -> [u8; 16] {
    addr[2..4].copy_from_slice(&port.to_be_bytes());
    addr
}

fn bind(fd: usize, addr: &[u8; 16]) -> isize {
    syscall(nr::BIND, [fd, addr.as_ptr() as usize, 16, 0, 0, 0])
}

fn connect(fd: usize, addr: &[u8]) -> isize {
    syscall(
        nr::CONNECT,
        [fd, addr.as_ptr() as usize, addr.len(), 0, 0, 0],
    )
}

fn udp_socket() -> usize {
    let s = syscall(nr::SOCKET, [2, SOCK_DGRAM | SOCK_NONBLOCK, 0, 0, 0, 0]);
    check(s >= 0, "udp socket");
    s as usize
}

fn sendto(fd: usize, bytes: &[u8], to: &[u8; 16]) -> isize {
    syscall(
        nr::SENDTO,
        [
            fd,
            bytes.as_ptr() as usize,
            bytes.len(),
            0,
            to.as_ptr() as usize,
            16,
        ],
    )
}

fn recv(fd: usize, buf: &mut [u8]) -> isize {
    syscall(
        more::RECVFROM,
        [fd, buf.as_mut_ptr() as usize, buf.len(), MSG_DONTWAIT, 0, 0],
    )
}

fn unlink(path: &core::ffi::CStr) {
    syscall(
        more::UNLINKAT,
        [AT_FDCWD, path.as_ptr() as usize, 0, 0, 0, 0],
    );
}

/// Total RAM, free RAM (bytes) and processes, as `sysinfo` gives them.
fn sysinfo() -> (u64, u64, u64) {
    let mut info = [0u8; 112];
    check(
        syscall(nr::SYSINFO, [info.as_mut_ptr() as usize, 0, 0, 0, 0, 0]) == 0,
        "sysinfo",
    );
    let word = |at: usize| u64::from_le_bytes(info[at..at + 8].try_into().unwrap());
    let unit = u64::from(u32::from_le_bytes(info[104..108].try_into().unwrap()));
    let procs = u16::from_le_bytes([info[80], info[81]]);
    (word(32) * unit, word(40) * unit, procs.into())
}

fn print_sysinfo() {
    let (total, free, procs) = sysinfo();
    print(b"sysinfo");
    field(b"total_kib", total >> 10);
    field(b"free_kib", free >> 10);
    field(b"procs", procs);
    print(b"\n");
}

/// Memory taken 1 MiB at a time, every page touched, until refused: the
/// mappings (`n` of them) and the refusal.
fn take_memory(maps: &mut [usize; STEPS]) -> (usize, isize) {
    let mut n = 0;
    while n < STEPS {
        let p = syscall(nr::MMAP, [0, MIB, 3, 0x22, usize::MAX, 0]);
        if p < 0 {
            return (n, p);
        }
        for page in (0..MIB).step_by(4096) {
            // SAFETY: inside the mapping just made.
            unsafe { ((p as usize + page) as *mut u8).write_volatile(1) };
        }
        maps[n] = p as usize;
        n += 1;
    }
    (n, 0)
}

fn give_memory(maps: &[usize]) {
    for &p in maps {
        syscall(nr::MUNMAP, [p, MIB, 0, 0, 0, 0]);
    }
}

/// A file `path` written 64 KiB at a time until refused: KiB written and
/// the refusal.
fn fill_file(path: &core::ffi::CStr) -> (u64, isize) {
    static CHUNK: [u8; 64 << 10] = [7; 64 << 10];
    let fd = syscall(
        nr::OPENAT,
        [AT_FDCWD, path.as_ptr() as usize, O_RDWR_CREAT, 0o600, 0, 0],
    );
    if fd < 0 {
        return (0, fd);
    }
    let mut written = 0u64;
    let refused = loop {
        let n = syscall(
            nr::WRITE,
            [fd as usize, CHUNK.as_ptr() as usize, CHUNK.len(), 0, 0, 0],
        );
        if n <= 0 {
            break n;
        }
        written += n as u64;
    };
    close(fd as usize);
    (written >> 10, refused)
}

/// Sockets made until refused, after those already in `fds[..n]`.
fn fill_sockets(fds: &mut [usize; SOCKETS], mut n: usize) -> (usize, isize) {
    while n < SOCKETS {
        let s = tcp_socket(0);
        if s < 0 {
            return (n, s);
        }
        fds[n] = s as usize;
        n += 1;
    }
    (n, 0)
}

/// Stacks of the probe's threads.
#[repr(C, align(16))]
struct Stacks([[u8; 16 << 10]; 4]);
static mut STACKS: Stacks = Stacks([[0; 16 << 10]; 4]);
static TIDS: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];
static SEEN: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];

extern "C" fn thread(i: usize) -> usize {
    let tid = syscall(nr::GETTID, [0; 6]) as u32;
    SEEN[i].store(tid, core::sync::atomic::Ordering::Relaxed);
    0
}

/// Four threads started and joined; the identifiers they saw.
fn threads() -> [u32; 4] {
    for (i, tid) in TIDS.iter().enumerate() {
        // SAFETY: only the address is taken; each stack serves one thread,
        // joined before the next round.
        let top = unsafe { core::ptr::addr_of_mut!(STACKS.0[i]) as usize } + (16 << 10);
        check(spawn_thread(thread, i, top, 0, tid) > 0, "clone");
    }
    for tid in &TIDS {
        join(tid);
    }
    SEEN.each_ref()
        .map(|s| s.load(core::sync::atomic::Ordering::Relaxed))
}

/// A connected TCP socket to `to` (through the card), or the error.
fn connect_out(to: &[u8; 16]) -> Result<usize, isize> {
    let s = tcp_socket(0);
    if s < 0 {
        return Err(s);
    }
    match connect(s as usize, to) {
        0 => Ok(s as usize),
        e => {
            close(s as usize);
            Err(e)
        }
    }
}

fn probe(outside: Option<[u8; 16]>) {
    print(b"ready\n");
    sleep_ns(3_600_000_000_000);
    // Twice, so that more of the sender's rounds overlap it.
    for _ in 0..2 {
        probe_once(outside);
    }
}

fn probe_once(outside: Option<[u8; 16]>) {
    print(b"ids");
    field(b"pid", syscall(nr::GETPID, [0; 6]) as u64);
    field(b"tid", syscall(nr::GETTID, [0; 6]) as u64);
    print(b"\n");
    print_sysinfo();
    print(b"threads");
    for tid in threads() {
        field(b"tid", tid.into());
    }
    print(b"\n");

    let mut maps = [0usize; STEPS];
    let (n, refused) = take_memory(&mut maps);
    print(b"memory");
    field(b"mib", n as u64);
    error(b"refused", refused);
    let (_, free, _) = sysinfo();
    field(b"free_kib", free >> 10);
    print(b"\n");
    give_memory(&maps[..n]);

    let (kib, refused) = fill_file(c"/data");
    print(b"file");
    field(b"kib", kib);
    error(b"refused", refused);
    print(b"\n");
    unlink(c"/data");

    let mut fds = [0usize; SOCKETS];
    let (n, refused) = fill_sockets(&mut fds, 0);
    print(b"sockets");
    field(b"n", n as u64);
    error(b"refused", refused);
    print(b"\n");
    fds[..n].iter().for_each(|&s| close(s));

    loopback_probe();
    ephemeral_probe();
    if let Some(to) = outside {
        print(b"outside");
        match connect_out(&to) {
            Ok(s) => {
                let msg = [b'r'; 64];
                let mut back = [0u8; 64];
                let ok = write_all(s, &msg) == 64 && read_full(s, &mut back) == 64 && back == msg;
                print(if ok { b" echo=ok" } else { b" echo=bad" });
                close(s);
            }
            Err(e) => error(b"refused", e),
        }
        print(b"\n");
    }
    pages_probe();
    print_sysinfo();
}

/// Binds, a bind conflict, a connection with one round trip, a refused
/// connection and a datagram, on the loopback ports both colluders use.
fn loopback_probe() {
    let l = tcp_socket(0);
    check(l >= 0, "socket");
    let l = l as usize;
    print(b"loopback");
    error(b"listen", tcp_listen(l, PORTS[0]));
    let again = tcp_socket(0) as usize;
    error(b"again", bind(again, &loopback(PORTS[0])));
    close(again);
    let c = tcp_socket(0) as usize;
    error(b"connect", connect(c, &loopback(PORTS[0])));
    let s = syscall(nr::ACCEPT4, [l, 0, 0, 0, 0, 0]);
    let msg = [b'l'; 64];
    let mut back = [0u8; 64];
    let ok =
        s >= 0 && write_all(c, &msg) == 64 && read_full(s as usize, &mut back) == 64 && back == msg;
    print(if ok { b" echo=ok" } else { b" echo=bad" });
    let other = tcp_socket(0) as usize;
    error(b"unheard", connect(other, &loopback(PORTS[1])));
    let u = udp_socket();
    error(b"udp_bind", bind(u, &loopback(PORTS[2])));
    let sent = sendto(u, b"datagram", &loopback(PORTS[2]));
    let mut buf = [0u8; 16];
    let got = recv(u, &mut buf);
    print(if sent == 8 && got == 8 && &buf[..8] == b"datagram" {
        b" udp=ok"
    } else {
        b" udp=bad"
    });
    print(b"\n");
    for fd in [l, c, other, u] {
        close(fd);
    }
    if s >= 0 {
        close(s as usize);
    }
}

/// Three binds to port 0 and the distances between their ports.
fn ephemeral_probe() {
    let mut ports = [0u16; 3];
    let mut fds = [0usize; 3];
    for (port, fd) in ports.iter_mut().zip(&mut fds) {
        let s = tcp_socket(0);
        check(s >= 0 && bind(s as usize, &loopback(0)) == 0, "bind");
        *fd = s as usize;
        *port = local_port(s as usize);
    }
    let span = 60999 - 32768 + 1;
    print(b"ephemeral");
    for w in ports.windows(2) {
        let d = (u32::from(w[1]) + span - u32::from(w[0])) % span;
        field(b"step", d.into());
    }
    print(b"\n");
    fds.iter().for_each(|&fd| close(fd));
}

/// A 16-page file of its own mapped twice: the sum of its bytes each time.
fn pages_probe() {
    const LEN: usize = 16 * 4096;
    let fd = syscall(
        nr::OPENAT,
        [
            AT_FDCWD,
            c"/pages".as_ptr() as usize,
            O_RDWR_CREAT,
            0o600,
            0,
            0,
        ],
    );
    check(fd >= 0, "open pages");
    let fd = fd as usize;
    let mut page = [0u8; 4096];
    for i in 0..16 {
        page.fill(i as u8 + 1);
        let at = i * 4096;
        check(
            syscall(nr::PWRITE64, [fd, page.as_ptr() as usize, 4096, at, 0, 0]) == 4096,
            "write pages",
        );
    }
    print(b"pages");
    for _ in 0..2 {
        let at = syscall(nr::MMAP, [0, LEN, 1, 2, fd, 0]);
        check(at > 0, "mmap pages");
        let mut sum = 0u64;
        for i in 0..LEN {
            // SAFETY: inside the readable mapping just made.
            sum += u64::from(unsafe { ((at as usize + i) as *const u8).read_volatile() });
        }
        field(b"sum", sum);
        syscall(nr::MUNMAP, [at as usize, LEN, 0, 0, 0, 0]);
    }
    print(b"\n");
    close(fd);
}

/// What one round of the sender holds.
struct Held {
    maps: [usize; STEPS],
    mapped: usize,
    fds: [usize; SOCKETS],
    sockets: usize,
}

fn send(outside: Option<[u8; 16]>) -> ! {
    let mut held = Held {
        maps: [0; STEPS],
        mapped: 0,
        fds: [0; SOCKETS],
        sockets: 0,
    };
    let mut round = 0u64;
    loop {
        // Connections out first, then the receiver's loopback ports, then
        // every socket left.
        while let Some(to) = outside.filter(|_| held.sockets < 4) {
            match connect_out(&to) {
                Ok(s) => {
                    held.fds[held.sockets] = s;
                    held.sockets += 1;
                }
                Err(_) => break,
            }
        }
        for port in PORTS {
            let s = tcp_socket(0);
            if s >= 0 {
                tcp_listen(s as usize, port);
                held.fds[held.sockets] = s as usize;
                held.sockets += 1;
            }
        }
        for _ in 0..100 {
            let s = tcp_socket(0);
            if s >= 0 {
                bind(s as usize, &loopback(0));
                close(s as usize);
            }
        }
        held.sockets = fill_sockets(&mut held.fds, held.sockets).0;
        // Memory and file data share its limit: one, then the other, by
        // turns.
        if round.is_multiple_of(2) {
            held.mapped = take_memory(&mut held.maps).0;
        }
        fill_file(c"/data");
        if !round.is_multiple_of(2) {
            held.mapped = take_memory(&mut held.maps).0;
        }
        // File pages: a file of its own mapped until refused (or 64 MiB).
        let fd = syscall(
            nr::OPENAT,
            [AT_FDCWD, c"/data".as_ptr() as usize, 0, 0, 0, 0],
        );
        let mut views = [0usize; 64];
        let mut viewed = 0;
        while fd >= 0 && viewed < views.len() {
            let at = syscall(nr::MMAP, [0, MIB, 1, 2, fd as usize, 0]);
            if at < 0 {
                break;
            }
            views[viewed] = at as usize;
            viewed += 1;
        }
        sysinfo();
        threads();
        if round == 0 {
            print(b"sending\n");
        } else {
            print(b".");
        }
        for _ in 0..4 {
            yield_now();
        }
        for &at in &views[..viewed] {
            syscall(nr::MUNMAP, [at, MIB, 0, 0, 0, 0]);
        }
        if fd >= 0 {
            close(fd as usize);
        }
        unlink(c"/data");
        give_memory(&held.maps[..held.mapped]);
        held.fds[..held.sockets].iter().for_each(|&s| close(s));
        held.sockets = 0;
        round += 1;
        yield_now();
    }
}

fn listen(port: u16) {
    let any = at_port(address(b"0.0.0.0"), port);
    let t = tcp_socket(SOCK_NONBLOCK);
    check(t >= 0, "socket");
    let t = t as usize;
    check(bind(t, &any) == 0, "bind tcp");
    check(syscall(nr::LISTEN, [t, 16, 0, 0, 0, 0]) == 0, "listen");
    let u = udp_socket();
    check(bind(u, &any) == 0, "bind udp");
    let x = syscall(nr::SOCKET, [1, 1 | SOCK_NONBLOCK, 0, 0, 0, 0]);
    check(x >= 0, "unix socket");
    let x = x as usize;
    let name = unix_name();
    check(
        syscall(nr::BIND, [x, name.as_ptr() as usize, name.len(), 0, 0, 0]) == 0,
        "bind unix",
    );
    check(syscall(nr::LISTEN, [x, 16, 0, 0, 0, 0]) == 0, "listen unix");
    print(b"listening\n");
    sleep_ns(3_600_000_000_000);
    let mut buf = [0u8; 16];
    let heard = syscall(nr::ACCEPT4, [t, 0, 0, 0, 0, 0]) != EAGAIN
        || recv(u, &mut buf) != EAGAIN
        || syscall(nr::ACCEPT4, [x, 0, 0, 0, 0, 0]) != EAGAIN;
    print(if heard {
        b"heard\n"
    } else {
        b"heard nothing\n"
    });
}

/// The `sockaddr_un` of `/colluder`.
fn unix_name() -> [u8; 12] {
    let mut name = [0u8; 12];
    name[0] = 1; // AF_UNIX
    name[2..11].copy_from_slice(b"/colluder");
    name
}

/// One letter for the result of a connection attempt or a send.
fn letter(r: isize) -> &'static [u8] {
    match r {
        ECONNREFUSED => b"R",
        ENOENT => b"N",
        EACCES => b"D",
        EALREADY | EINPROGRESS => b"W",
        0 => b"C",
        r if r > 0 => b"S",
        _ => b"?",
    }
}

fn knock(port: u16, card: Option<&[u8]>) {
    let to = loopback(port);
    let t = tcp_socket(0) as usize;
    print(letter(connect(t, &to)));
    close(t);
    let u = udp_socket();
    print(letter(sendto(u, b"knock", &to)));
    let x = syscall(nr::SOCKET, [1, 1, 0, 0, 0, 0]) as usize;
    print(letter(connect(x, &unix_name())));
    close(x);
    if let Some(card) = card {
        let at = at_port(address(card), port);
        let t = tcp_socket(SOCK_NONBLOCK) as usize;
        let r = connect(t, &at);
        // A connection the card starts is not made yet: give it time.
        if r == EINPROGRESS {
            sleep_ns(200_000_000);
            print(letter(connect(t, &at)));
        } else {
            print(letter(r));
        }
        close(t);
        print(letter(sendto(u, b"knock", &at)));
    }
    close(u);
    print(b"\n");
    let l = tcp_socket(0);
    check(l >= 0 && tcp_listen(l as usize, port) == 0, "bind its port");
    print(b"bound\n");
}

fn dial(to: &[u8; 16]) {
    print(b"ready\n");
    sleep_ns(3_600_000_000_000);
    let t = tcp_socket(0) as usize;
    print(letter(connect(t, to)));
    print(b"\n");
}

/// The fixed bit sequence both ends of the channel know.
fn bit(i: u64) -> bool {
    let mut x = 0x9e37_79b9_7f4a_7c15u64 ^ i.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 31;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 29;
    x & 1 == 1
}

fn sleep_until(at: u64) {
    let ts = [(at / 1_000_000_000) as i64, (at % 1_000_000_000) as i64];
    // CLOCK_MONOTONIC, TIMER_ABSTIME.
    while syscall(more::CLOCK_NANOSLEEP, [1, 1, ts.as_ptr() as usize, 0, 0, 0]) != 0 {}
}

fn bits(start: u64, period: u64, n: u64) {
    sleep_until(start);
    for i in 0..n {
        let end = start + (i + 1) * period;
        if bit(i) {
            while now_ns() < end {}
        } else {
            sleep_until(end);
        }
    }
}

fn count(start: u64, period: u64, n: u64) {
    const MOST: usize = 1024;
    check(n as usize <= MOST, "periods");
    let mut counts = [0u64; MOST];
    sleep_until(start);
    for (i, c) in counts.iter_mut().take(n as usize).enumerate() {
        let end = start + (i as u64 + 1) * period;
        while now_ns() < end {
            *c += 1;
        }
    }
    let counts = &counts[..n as usize];
    // Against the median of the periods around it (half the bits are
    // 1s), which follows the drift of the CPU's speed.
    let errors = (0..counts.len())
        .filter(|&i| {
            let near = &counts[i.saturating_sub(4)..(i + 5).min(counts.len())];
            let mut sorted = [0u64; 9];
            sorted[..near.len()].copy_from_slice(near);
            let sorted = &mut sorted[..near.len()];
            sorted.sort_unstable();
            (counts[i] < sorted[sorted.len() / 2]) != bit(i as u64)
        })
        .count();
    print(b"bits");
    field(b"n", n);
    field(b"errors", errors as u64);
    print(b"\n");
}

fn main() -> i32 {
    let num = |i| arg(i).map_or(0, number) as u64;
    match arg(1) {
        Some(b"probe") => probe(arg(2).map(address)),
        Some(b"send") => send(arg(2).map(address)),
        Some(b"listen") => listen(num(2) as u16),
        Some(b"knock") => knock(num(2) as u16, arg(3)),
        Some(b"dial") => dial(&address(arg(2).unwrap_or_default())),
        Some(b"bits") => bits(num(2), num(3), num(4)),
        Some(b"count") => count(num(2), num(3), num(4)),
        _ => check(false, "mode"),
    }
    0
}

rhizo_cells::entry!(main);
