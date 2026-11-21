//! Host tests of pipes, event counters, waiting on descriptors and socket
//! calls, through the system-call entry point (AArch64 numbering unless the
//! world is x86_64).

use super::tests::{DATA, World};
use super::*;

const PIPE2: u64 = 59;
const READ: u64 = 63;
const WRITE: u64 = 64;
const READV: u64 = 65;
const CLOSE: u64 = 57;
const FCNTL: u64 = 25;
const NEWFSTATAT: u64 = 79;
const PSELECT6: u64 = 72;
const PPOLL: u64 = 73;
const EVENTFD2: u64 = 19;
const EPOLL_CREATE1: u64 = 20;
const EPOLL_CTL: u64 = 21;
const EPOLL_PWAIT: u64 = 22;
const SOCKET: u64 = 198;
const SOCKETPAIR: u64 = 199;
const BIND: u64 = 200;
const LISTEN: u64 = 201;
const ACCEPT4: u64 = 242;
const CONNECT: u64 = 203;
const GETSOCKNAME: u64 = 204;
const GETPEERNAME: u64 = 205;
const SENDTO: u64 = 206;
const RECVFROM: u64 = 207;
const SETSOCKOPT: u64 = 208;
const GETSOCKOPT: u64 = 209;
const SHUTDOWN: u64 = 210;
const SENDMSG: u64 = 211;
const RECVMSG: u64 = 212;

const O_NONBLOCK: u64 = 0x800;
const POLLIN: u16 = 1;
const POLLOUT: u16 = 4;
const POLLHUP: u16 = 0x10;
const POLLNVAL: u16 = 0x20;
const EPOLLIN: u32 = 1;
const EPOLLONESHOT: u32 = 1 << 30;
const AT_FDCWD: u64 = -100i64 as u64;
const MS: u64 = 1_000_000;

/// Scratch addresses inside the writable page.
const BUF: u64 = DATA + 0x100;
const ADDR: u64 = DATA + 0x400;
const LEN: u64 = DATA + 0x480;
const OUT: u64 = DATA + 0x500;
const SEND: u64 = DATA + 0x800;

fn sin(ip: [u8; 4], port: u16) -> Vec<u8> {
    let mut a = vec![2, 0];
    a.extend_from_slice(&port.to_be_bytes());
    a.extend_from_slice(&ip);
    a.extend_from_slice(&[0; 8]);
    a
}

fn sun(path: &[u8]) -> Vec<u8> {
    let mut a = vec![1, 0];
    a.extend_from_slice(path);
    a.push(0);
    a
}

fn int(v: i32) -> [u8; 4] {
    v.to_le_bytes()
}

impl World {
    /// Makes a call that must wait on descriptors; returns its deadline
    /// (`Some(None)`: none), or `None` if it did not wait.
    fn waits(&mut self, nr: u64, args: &[u64]) -> Option<Option<u64>> {
        let (_, out) = self.call(nr, args);
        assert_eq!(out, Outcome::Block, "the call waits");
        self.io
    }

    fn fds_at(&self, addr: u64) -> (u64, u64) {
        let b = self.peek(addr, 8);
        (
            u64::from(u32::from_le_bytes(b[..4].try_into().unwrap())),
            u64::from(u32::from_le_bytes(b[4..].try_into().unwrap())),
        )
    }

    fn send(&mut self, fd: u64, data: &[u8]) -> i64 {
        self.poke(SEND, data);
        self.call(WRITE, &[fd, SEND, data.len() as u64]).0
    }

    fn recv(&mut self, fd: u64, len: u64) -> Result<Vec<u8>, i64> {
        match self.call(READ, &[fd, BUF, len]) {
            (n, Outcome::Resume) if n >= 0 => Ok(self.peek(BUF, n as usize)),
            (n, Outcome::Resume) => Err(n),
            (_, other) => panic!("read did not complete: {other:?}"),
        }
    }

    fn tcp_listener(&mut self, port: u16) -> u64 {
        let l = self.call(SOCKET, &[2, 1, 0]).0 as u64;
        self.poke(ADDR, &sin([127, 0, 0, 1], port));
        assert_eq!(self.call(BIND, &[l, ADDR, 16]).0, 0);
        assert_eq!(self.call(LISTEN, &[l, 8]).0, 0);
        l
    }

    fn tcp_connect(&mut self, port: u16) -> u64 {
        let c = self.call(SOCKET, &[2, 1, 0]).0 as u64;
        self.poke(ADDR, &sin([127, 0, 0, 1], port));
        assert_eq!(self.call(CONNECT, &[c, ADDR, 16]).0, 0);
        c
    }
}

#[test]
fn pipes_carry_bytes_and_block_when_empty() {
    let mut w = World::new(Arch::Aarch64);
    assert_eq!(w.call(PIPE2, &[OUT, 0]).0, 0);
    let (r, wr) = w.fds_at(OUT);
    assert_eq!((r, wr), (3, 4));
    assert_eq!(w.send(wr, b"abc"), 3);
    assert_eq!(w.recv(r, 10), Ok(b"abc".to_vec()));
    let pc = w.cell.regs().pc;
    assert_eq!(w.waits(READ, &[r, BUF, 10]), Some(None));
    assert_eq!(w.cell.regs().pc, pc - 4, "back on the SVC instruction");
    assert_eq!(w.cell.regs().gpr[0], r, "the arguments are kept");
    assert_eq!(w.call(FCNTL, &[r, 4, O_NONBLOCK]).0, 0);
    assert_eq!(w.call(FCNTL, &[r, 3, 0]).0, O_NONBLOCK as i64);
    assert_eq!(w.recv(r, 10), Err(-EAGAIN));
    assert_eq!(w.call(CLOSE, &[wr]).0, 0);
    assert_eq!(w.recv(r, 10), Ok(vec![]), "end of the pipe");
    assert_eq!(w.call(PIPE2, &[OUT, 0x4000]).0, -EINVAL, "unknown flag");
    assert_eq!(w.call(PIPE2, &[DATA + 0x11_0000, 0]).0, -EFAULT);
    assert_eq!(
        w.call(READ, &[5, BUF, 1]).0,
        -EBADF,
        "a failed pipe2 leaves nothing open"
    );
}

#[test]
fn pipes_are_not_seekable_and_report_epipe() {
    let mut w = World::new(Arch::Aarch64);
    w.call(PIPE2, &[OUT, O_NONBLOCK]);
    let (r, wr) = w.fds_at(OUT);
    assert_eq!(w.call(62, &[r, 0, 0]).0, -ESPIPE, "lseek");
    assert_eq!(w.call(67, &[r, BUF, 1, 0]).0, -ESPIPE, "pread64");
    w.call(CLOSE, &[r]);
    assert_eq!(w.send(wr, b"x"), -EPIPE);
    w.poke(OUT, b"\0");
    assert_eq!(w.call(NEWFSTATAT, &[wr, OUT, BUF, 0x1000]).0, 0);
    let mode = u32::from_le_bytes(w.peek(BUF + 16, 4).try_into().unwrap());
    assert_eq!(mode & 0o170_000, 0o010_000, "a FIFO");
}

#[test]
fn readv_gathers_from_a_pipe() {
    let mut w = World::new(Arch::Aarch64);
    w.call(PIPE2, &[OUT, 0]);
    let (r, wr) = w.fds_at(OUT);
    w.send(wr, b"abcdef");
    let iov = [BUF + 0x40, 2, BUF + 0x80, 10]
        .map(u64::to_le_bytes)
        .concat();
    w.poke(BUF, &iov);
    assert_eq!(w.call(READV, &[r, BUF, 2]).0, 6);
    assert_eq!(w.peek(BUF + 0x40, 2), b"ab");
    assert_eq!(w.peek(BUF + 0x80, 4), b"cdef");
}

#[test]
fn event_counters_count() {
    let mut w = World::new(Arch::Aarch64);
    let e = w.call(EVENTFD2, &[5, O_NONBLOCK]).0 as u64;
    w.poke(BUF, &3u64.to_le_bytes());
    assert_eq!(w.call(WRITE, &[e, BUF, 8]).0, 8);
    assert_eq!(w.call(READ, &[e, OUT, 8]).0, 8);
    assert_eq!(w.peek(OUT, 8), 8u64.to_le_bytes());
    assert_eq!(w.call(READ, &[e, OUT, 8]).0, -EAGAIN);
    assert_eq!(w.call(READ, &[e, OUT, 4]).0, -EINVAL, "less than 8 bytes");
    let s = w.call(EVENTFD2, &[2, 1]).0 as u64; // EFD_SEMAPHORE
    assert_eq!(w.call(READ, &[s, OUT, 8]).0, 8);
    assert_eq!(w.peek(OUT, 8), 1u64.to_le_bytes());
}

#[test]
fn poll_waits_until_ready_or_its_deadline() {
    let mut w = World::new(Arch::Aarch64);
    w.call(PIPE2, &[OUT, 0]);
    let (r, wr) = w.fds_at(OUT);
    let pollfds = |fds: &[(i32, u16)]| -> Vec<u8> {
        fds.iter()
            .flat_map(|&(fd, ev)| [&fd.to_le_bytes()[..], &ev.to_le_bytes(), &[0, 0]].concat())
            .collect()
    };
    w.poke(
        BUF,
        &pollfds(&[
            (r as i32, POLLIN),
            (wr as i32, POLLOUT),
            (-1, POLLIN),
            (9, POLLIN),
        ]),
    );
    assert_eq!(
        w.call(PPOLL, &[BUF, 4, 0]).0,
        2,
        "write end and the bad descriptor"
    );
    assert_eq!(w.peek(BUF + 6, 2), [0, 0]);
    assert_eq!(w.peek(BUF + 14, 2), POLLOUT.to_le_bytes());
    assert_eq!(
        w.peek(BUF + 22, 2),
        [0, 0],
        "negative descriptors are skipped"
    );
    assert_eq!(w.peek(BUF + 30, 2), POLLNVAL.to_le_bytes());

    w.poke(BUF, &pollfds(&[(r as i32, POLLIN)]));
    w.poke(
        OUT,
        &[0i64.to_le_bytes(), (10 * MS as i64).to_le_bytes()].concat(),
    );
    w.services.clock = 1000;
    assert_eq!(w.waits(PPOLL, &[BUF, 1, OUT]), Some(Some(1000 + 10 * MS)));
    w.services.clock = 5 * MS;
    assert_eq!(
        w.waits(PPOLL, &[BUF, 1, OUT]),
        Some(Some(1000 + 10 * MS)),
        "the deadline is kept when the call is made again"
    );
    w.services.clock = 1000 + 10 * MS;
    assert_eq!(
        w.call(PPOLL, &[BUF, 1, OUT]),
        (0, Outcome::Resume),
        "timed out"
    );

    w.send(wr, b"x");
    w.call(CLOSE, &[wr]);
    assert_eq!(w.call(PPOLL, &[BUF, 1, 0]).0, 1);
    assert_eq!(
        w.peek(BUF + 6, 2),
        (POLLIN | POLLHUP).to_le_bytes(),
        "only what was asked, and hang-ups"
    );
}

#[test]
fn select_reports_ready_descriptors() {
    let mut w = World::new(Arch::Aarch64);
    w.call(PIPE2, &[OUT, 0]);
    let (r, wr) = w.fds_at(OUT);
    let set = |fds: &[u64]| fds.iter().fold(0u64, |s, &fd| s | 1 << fd).to_le_bytes();
    w.poke(BUF, &set(&[r]));
    w.poke(BUF + 8, &set(&[wr]));
    w.poke(OUT + 16, &[0u8; 16]); // zero timeout
    assert_eq!(w.call(PSELECT6, &[8, BUF, BUF + 8, 0, OUT + 16, 0]).0, 1);
    assert_eq!(w.peek(BUF, 8), set(&[]), "the empty pipe is not readable");
    assert_eq!(w.peek(BUF + 8, 8), set(&[wr]));
    w.poke(BUF, &set(&[r, 9]));
    assert_eq!(w.call(PSELECT6, &[10, BUF, 0, 0, OUT + 16, 0]).0, -EBADF);
}

#[test]
fn epoll_reports_ready_entries_with_their_data() {
    for arch in [Arch::Aarch64, Arch::X86_64] {
        let mut w = World::new(arch);
        let nr = |a64: u64, x86: u64| if arch == Arch::Aarch64 { a64 } else { x86 };
        let (size, at) = if arch == Arch::Aarch64 {
            (16, 8)
        } else {
            (12, 4)
        };
        w.call(nr(PIPE2, 293), &[OUT, 0]);
        let (r, wr) = w.fds_at(OUT);
        let ep = w.call(nr(EPOLL_CREATE1, 291), &[0]).0 as u64;
        let mut event = vec![0u8; size];
        event[..4].copy_from_slice(&(EPOLLIN | EPOLLONESHOT).to_le_bytes());
        event[at..at + 8].copy_from_slice(&0x1234_5678_9abc_u64.to_le_bytes());
        w.poke(BUF, &event);
        assert_eq!(w.call(nr(EPOLL_CTL, 233), &[ep, 1, r, BUF]).0, 0);
        assert_eq!(
            w.call(nr(EPOLL_CTL, 233), &[ep, 1, r, BUF]).0,
            -17,
            "EEXIST"
        );
        assert_eq!(w.call(nr(EPOLL_PWAIT, 281), &[ep, OUT, 4, 0, 0]).0, 0);
        assert_eq!(
            w.waits(nr(EPOLL_PWAIT, 281), &[ep, OUT, 4, 20, 0]),
            Some(Some(20 * MS))
        );
        w.deadline = None;
        w.poke(BUF + 0x40, b"x");
        w.call(nr(WRITE, 1), &[wr, BUF + 0x40, 1]);
        assert_eq!(
            w.call(nr(EPOLL_PWAIT, 281), &[ep, OUT, 4, u64::MAX, 0]).0,
            1
        );
        assert_eq!(w.peek(OUT, 4), EPOLLIN.to_le_bytes());
        assert_eq!(
            w.peek(OUT + at as u64, 8),
            0x1234_5678_9abc_u64.to_le_bytes()
        );
        assert_eq!(
            w.call(nr(EPOLL_PWAIT, 281), &[ep, OUT, 4, 0, 0]).0,
            0,
            "one-shot: disabled after it fired"
        );
        assert_eq!(w.call(nr(EPOLL_PWAIT, 281), &[ep, OUT, 0, 0, 0]).0, -EINVAL);
        w.fs.put_file(b"/f", b"").unwrap();
        let f =
            w.fs.open(None, b"/f", rhizo_fs::OpenFlags::default(), 0)
                .unwrap();
        assert_eq!(
            w.call(nr(EPOLL_CTL, 233), &[ep, 1, u64::from(f), BUF]).0,
            -1,
            "EPERM"
        );
    }
}

#[test]
fn tcp_over_loopback_in_one_cell() {
    let mut w = World::new(Arch::Aarch64);
    let l = w.tcp_listener(7000);
    let c = w.tcp_connect(7000);
    w.poke(LEN, &int(16));
    let s = w.call(ACCEPT4, &[l, ADDR, LEN, 0]).0 as u64;
    assert!(s > c, "a new descriptor: {s}");
    let peer = w.peek(ADDR, 16);
    assert_eq!(&peer[..2], &[2, 0]);
    assert_eq!(&peer[4..8], &[127, 0, 0, 1]);
    assert_eq!(w.peek(LEN, 4), int(16));
    assert_eq!(w.send(c, b"ping"), 4);
    assert_eq!(w.recv(s, 100), Ok(b"ping".to_vec()));
    assert_eq!(
        w.waits(READ, &[s, BUF, 100]),
        Some(None),
        "nothing more yet"
    );
    w.poke(LEN, &int(16));
    assert_eq!(w.call(GETSOCKNAME, &[s, ADDR, LEN]).0, 0);
    assert_eq!(w.peek(ADDR, 8), sin([127, 0, 0, 1], 7000)[..8]);
    w.poke(LEN, &int(16));
    assert_eq!(w.call(GETPEERNAME, &[c, ADDR, LEN]).0, 0);
    assert_eq!(w.peek(ADDR, 8), sin([127, 0, 0, 1], 7000)[..8]);
    assert_eq!(w.call(SHUTDOWN, &[c, 1]).0, 0);
    assert_eq!(w.recv(s, 100), Ok(vec![]), "end of the stream");
    assert_eq!(w.call(GETPEERNAME, &[l, ADDR, LEN]).0, -ENOTCONN);
    let open = w.net.open();
    w.call(CLOSE, &[s]);
    assert_eq!(
        w.net.open(),
        open - 1,
        "closing the last descriptor closes the socket"
    );
    w.call(FCNTL, &[l, 4, O_NONBLOCK]);
    assert_eq!(w.call(ACCEPT4, &[l, 0, 0, 0]).0, -EAGAIN);
}

#[test]
fn tcp_between_two_cells_shares_the_network() {
    let mut server = World::new(Arch::Aarch64);
    let mut client = World::new(Arch::X86_64);
    let l = server.tcp_listener(8080);
    core::mem::swap(&mut server.net, &mut client.net);
    let c = client.call(41, &[2, 1, 0]).0 as u64; // x86_64 socket
    client.poke(ADDR, &sin([127, 0, 0, 1], 8080));
    assert_eq!(client.call(42, &[c, ADDR, 16]).0, 0); // x86_64 connect
    client.poke(BUF, b"hello");
    assert_eq!(client.call(1, &[c, BUF, 5]).0, 5);
    core::mem::swap(&mut server.net, &mut client.net);
    let s = server.call(ACCEPT4, &[l, 0, 0, 0]).0 as u64;
    assert_eq!(server.recv(s, 10), Ok(b"hello".to_vec()));
}

#[test]
fn non_blocking_connect_is_in_progress_and_writable() {
    let mut w = World::new(Arch::Aarch64);
    w.tcp_listener(7001);
    let c = w.call(SOCKET, &[2, 1 | O_NONBLOCK, 0]).0 as u64;
    w.poke(ADDR, &sin([127, 0, 0, 1], 7001));
    assert_eq!(w.call(CONNECT, &[c, ADDR, 16]).0, -EINPROGRESS);
    assert_eq!(w.call(CONNECT, &[c, ADDR, 16]).0, -106, "EISCONN");
    w.poke(LEN, &int(4));
    assert_eq!(w.call(GETSOCKOPT, &[c, 1, 4, OUT, LEN]).0, 0, "SO_ERROR");
    assert_eq!(w.peek(OUT, 4), int(0));
}

#[test]
fn socket_errors_follow_linux() {
    let mut w = World::new(Arch::Aarch64);
    assert_eq!(w.call(SOCKET, &[10, 1, 0]).0, -EAFNOSUPPORT, "IPv6");
    assert_eq!(w.call(SOCKET, &[2, 3, 0]).0, -ESOCKTNOSUPPORT, "raw");
    assert_eq!(
        w.call(SOCKET, &[2, 1, 17]).0,
        -EPROTONOSUPPORT,
        "TCP over UDP"
    );
    let c = w.call(SOCKET, &[2, 1, 0]).0 as u64;
    w.poke(ADDR, &sin([127, 0, 0, 1], 9));
    assert_eq!(w.call(CONNECT, &[c, ADDR, 16]).0, -111, "ECONNREFUSED");
    w.poke(ADDR, &sin([10, 0, 0, 1], 9));
    assert_eq!(w.call(CONNECT, &[c, ADDR, 16]).0, -101, "ENETUNREACH");
    assert_eq!(w.call(BIND, &[c, ADDR, 16]).0, -99, "EADDRNOTAVAIL");
    assert_eq!(w.call(CONNECT, &[c, ADDR, 8]).0, -EINVAL, "short address");
    assert_eq!(w.call(LISTEN, &[0, 1]).0, -ENOTSOCK);
    assert_eq!(w.call(LISTEN, &[77, 1]).0, -EBADF);
    assert_eq!(w.call(READ, &[c, BUF, 1]).0, -ENOTCONN);
    w.tcp_listener(9);
    let d = w.call(SOCKET, &[2, 1, 0]).0 as u64;
    w.poke(ADDR, &sin([127, 0, 0, 1], 9));
    assert_eq!(w.call(BIND, &[d, ADDR, 16]).0, -EADDRINUSE);
}

#[test]
fn udp_datagrams_with_addresses() {
    let mut w = World::new(Arch::Aarch64);
    let a = w.call(SOCKET, &[2, 2, 0]).0 as u64;
    let b = w.call(SOCKET, &[2, 2, 0]).0 as u64;
    w.poke(ADDR, &sin([127, 0, 0, 1], 5353));
    assert_eq!(w.call(BIND, &[b, ADDR, 16]).0, 0);
    w.poke(BUF, b"datagram");
    assert_eq!(w.call(SENDTO, &[a, BUF, 8, 0, ADDR, 16]).0, 8);
    w.poke(LEN, &int(16));
    assert_eq!(
        w.call(RECVFROM, &[b, OUT, 4, 0x20, ADDR + 32, LEN]).0,
        8,
        "MSG_TRUNC: full length"
    );
    assert_eq!(w.peek(OUT, 4), b"data");
    assert_eq!(w.peek(ADDR + 32 + 4, 4), [127, 0, 0, 1]);
    // sendmsg with two buffers, recvmsg into two buffers.
    let iov = [BUF, 3, BUF + 3, 5].map(u64::to_le_bytes).concat();
    w.poke(DATA + 0x600, &iov);
    let mut hdr = vec![0u8; 56];
    hdr[..8].copy_from_slice(&ADDR.to_le_bytes());
    hdr[8..12].copy_from_slice(&16u32.to_le_bytes());
    hdr[16..24].copy_from_slice(&(DATA + 0x600).to_le_bytes());
    hdr[24..32].copy_from_slice(&2u64.to_le_bytes());
    w.poke(DATA + 0x700, &hdr);
    assert_eq!(w.call(SENDMSG, &[a, DATA + 0x700, 0]).0, 8);
    let iov = [OUT, 2, OUT + 0x10, 2].map(u64::to_le_bytes).concat();
    w.poke(DATA + 0x600, &iov);
    hdr[..8].copy_from_slice(&(ADDR + 64).to_le_bytes());
    w.poke(DATA + 0x700, &hdr);
    assert_eq!(w.call(RECVMSG, &[b, DATA + 0x700, 0]).0, 4);
    assert_eq!(w.peek(OUT, 2), b"da");
    assert_eq!(w.peek(OUT + 0x10, 2), b"ta");
    assert_eq!(
        w.peek(DATA + 0x700 + 48, 4),
        0x20u32.to_le_bytes(),
        "MSG_TRUNC"
    );
    assert_eq!(
        w.peek(DATA + 0x700 + 8, 4),
        16u32.to_le_bytes(),
        "name length"
    );
    assert_eq!(
        w.call(RECVFROM, &[b, OUT, 4, 0x40, 0, 0]).0,
        -EAGAIN,
        "MSG_DONTWAIT"
    );
}

#[test]
fn unix_sockets_are_named_by_files() {
    let mut w = World::new(Arch::Aarch64);
    let l = w.call(SOCKET, &[1, 1, 0]).0 as u64;
    let path = sun(b"sock");
    w.poke(ADDR, &path);
    assert_eq!(w.call(BIND, &[l, ADDR, path.len() as u64]).0, 0);
    assert_eq!(w.call(LISTEN, &[l, 4]).0, 0);
    w.poke(OUT, b"sock\0");
    assert_eq!(w.call(NEWFSTATAT, &[AT_FDCWD, OUT, BUF, 0]).0, 0);
    let mode = u32::from_le_bytes(w.peek(BUF + 16, 4).try_into().unwrap());
    assert_eq!(mode & 0o170_000, 0o140_000, "a socket file");
    let c = w.call(SOCKET, &[1, 1, 0]).0 as u64;
    assert_eq!(w.call(CONNECT, &[c, ADDR, path.len() as u64]).0, 0);
    let s = w.call(ACCEPT4, &[l, 0, 0, 0]).0 as u64;
    assert_eq!(w.send(s, b"unix"), 4);
    assert_eq!(w.recv(c, 10), Ok(b"unix".to_vec()));
    w.poke(LEN, &int(110));
    assert_eq!(w.call(GETPEERNAME, &[c, OUT, LEN]).0, 0);
    assert_eq!(w.peek(OUT, path.len()), path);
    let again = w.call(SOCKET, &[1, 1, 0]).0 as u64;
    assert_eq!(
        w.call(BIND, &[again, ADDR, path.len() as u64]).0,
        -EADDRINUSE
    );
    w.call(CLOSE, &[l]);
    assert_eq!(w.call(CONNECT, &[again, ADDR, path.len() as u64]).0, -111);
    let other = sun(b"none");
    w.poke(ADDR, &other);
    assert_eq!(
        w.call(CONNECT, &[again, ADDR, other.len() as u64]).0,
        -ENOENT
    );
    assert_eq!(w.call(SOCKETPAIR, &[1, 1, 0, OUT]).0, 0);
    let (x, y) = w.fds_at(OUT);
    assert_eq!(w.send(x, b"pair"), 4);
    assert_eq!(w.recv(y, 10), Ok(b"pair".to_vec()));
}

#[test]
fn socket_options_are_kept_and_reported() {
    let mut w = World::new(Arch::Aarch64);
    let s = w.call(SOCKET, &[2, 1, 0]).0 as u64;
    let get = |w: &mut World, level: u64, name: u64| -> i32 {
        w.poke(LEN, &int(4));
        assert_eq!(w.call(GETSOCKOPT, &[s, level, name, OUT, LEN]).0, 0);
        i32::from_le_bytes(w.peek(OUT, 4).try_into().unwrap())
    };
    w.poke(BUF, &int(1));
    assert_eq!(w.call(SETSOCKOPT, &[s, 1, 2, BUF, 4]).0, 0);
    assert_eq!(get(&mut w, 1, 2), 1, "SO_REUSEADDR");
    w.poke(BUF, &int(65536));
    assert_eq!(w.call(SETSOCKOPT, &[s, 1, 8, BUF, 4]).0, 0);
    assert_eq!(
        get(&mut w, 1, 8),
        131_072,
        "SO_RCVBUF is doubled, as Linux does"
    );
    assert_eq!(get(&mut w, 1, 3), 1, "SO_TYPE");
    assert_eq!(get(&mut w, 1, 39), 2, "SO_DOMAIN");
    assert_eq!(get(&mut w, 1, 30), 0, "SO_ACCEPTCONN");
    w.poke(BUF, &int(1));
    assert_eq!(w.call(SETSOCKOPT, &[s, 6, 1, BUF, 4]).0, 0);
    assert_eq!(get(&mut w, 6, 1), 1, "TCP_NODELAY");
    assert_eq!(w.call(SETSOCKOPT, &[s, 1, 99, BUF, 4]).0, -ENOPROTOOPT);
    assert_eq!(
        w.call(SETSOCKOPT, &[s, 1, 2, BUF, 2]).0,
        -EINVAL,
        "short value"
    );
}

#[test]
fn receive_timeout_bounds_a_blocking_receive() {
    let mut w = World::new(Arch::Aarch64);
    let l = w.tcp_listener(7002);
    let _c = w.tcp_connect(7002);
    let s = w.call(ACCEPT4, &[l, 0, 0, 0]).0 as u64;
    w.poke(BUF, &[0i64.to_le_bytes(), 10_000i64.to_le_bytes()].concat()); // 10 ms
    assert_eq!(w.call(SETSOCKOPT, &[s, 1, 20, BUF, 16]).0, 0);
    w.services.clock = 7;
    assert_eq!(w.waits(READ, &[s, BUF, 1]), Some(Some(7 + 10 * MS)));
    w.services.clock = 7 + 10 * MS;
    assert_eq!(w.call(READ, &[s, BUF, 1]).0, -EAGAIN, "timed out");
}

#[test]
fn only_calls_on_sockets_reach_them() {
    let mut w = World::new(Arch::Aarch64);
    w.call(PIPE2, &[OUT, 0]);
    let (r, wr) = w.fds_at(OUT);
    w.send(wr, b"abc");
    assert_eq!(w.call(READ, &[r, BUF, 3]).0, 3);
    let e = w.call(EVENTFD2, &[1, 0]).0 as u64;
    let ep = w.call(EPOLL_CREATE1, &[0]).0 as u64;
    w.poke(BUF, &[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(w.call(EPOLL_CTL, &[ep, 1, e, BUF]).0, 0);
    assert_eq!(w.call(EPOLL_PWAIT, &[ep, BUF, 4, 0, 0]).0, 1);
    assert!(!w.used_net, "pipes, counters and epoll without sockets");
    assert!(w.call(SOCKET, &[2, 1, 0]).0 >= 0);
    assert!(w.used_net);
}

/// Hostile (finding S25): what another cell binds does not move a cell's
/// ephemeral ports. Each cell's calls take them from its own cursor
/// (design D2 of `close-cross-cell-leaks`); with the one counter all cells
/// shared, the observer's second port would be 101 past its first.
#[test]
fn another_cells_binds_do_not_show_in_my_ports() {
    fn bound(w: &mut World) -> (u64, u16) {
        let s = w.call(SOCKET, &[2, 1, 0]).0 as u64;
        w.poke(ADDR, &sin([127, 0, 0, 1], 0));
        assert_eq!(w.call(BIND, &[s, ADDR, 16]).0, 0);
        w.poke(LEN, &int(16));
        assert_eq!(w.call(GETSOCKNAME, &[s, OUT, LEN]).0, 0);
        let a = w.peek(OUT, 4);
        (s, u16::from_be_bytes([a[2], a[3]]))
    }
    let mut observer = World::new(Arch::Aarch64);
    let mut neighbour = World::new(Arch::Aarch64);
    neighbour.cell.id = 6;
    let (_, first) = bound(&mut observer);
    core::mem::swap(&mut observer.net, &mut neighbour.net);
    for _ in 0..100 {
        let (s, _) = bound(&mut neighbour);
        assert_eq!(neighbour.call(CLOSE, &[s]).0, 0);
    }
    core::mem::swap(&mut observer.net, &mut neighbour.net);
    let (_, second) = bound(&mut observer);
    let next = if first == *rhizo_net::EPHEMERAL.end() {
        *rhizo_net::EPHEMERAL.start()
    } else {
        first + 1
    };
    assert_eq!(second, next);
    // A listener and a connection take theirs from the same cursor.
    let l = observer.call(SOCKET, &[2, 1, 0]).0 as u64;
    assert_eq!(observer.call(LISTEN, &[l, 8]).0, 0);
    observer.poke(LEN, &int(16));
    assert_eq!(observer.call(GETSOCKNAME, &[l, OUT, LEN]).0, 0);
    let a = observer.peek(OUT, 4);
    assert_eq!(u16::from_be_bytes([a[2], a[3]]), next + 1);
}
