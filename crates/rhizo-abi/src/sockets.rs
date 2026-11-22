//! Socket calls (spec `linux-abi`: socket calls and options; spec
//! `networking`), run against the kernel's [`rhizo_net::Net`]. A descriptor
//! holds a socket handle; Unix socket names are files of type socket in the
//! cell's tree.

use super::*;
use crate::files::err;
use crate::poll::{with_user_bytes, would_wait};
use alloc::vec;
use alloc::vec::Vec;
use rhizo_net::{Addr, Error as NetError, Family, Handle, Shutdown, Type};

const AF_UNIX: u16 = 1;
const AF_INET: u16 = 2;
const SOCK_NONBLOCK: u64 = 0x800;
const SOCK_CLOEXEC: u64 = 0x8_0000;
pub(crate) const MSG_PEEK: u64 = 0x2;
pub(crate) const MSG_TRUNC: u64 = 0x20;
pub(crate) const MSG_DONTWAIT: u64 = 0x40;
const SOL_SOCKET: u64 = 1;
const IPPROTO_IP: u64 = 0;
const IPPROTO_TCP: u64 = 6;
const IPPROTO_UDP: u64 = 17;
/// Linux's `net.core.rmem_max` and `wmem_max`.
const BUF_MAX: u32 = 212_992;
/// `struct msghdr` (the same layout on both architectures).
const MSGHDR: usize = 56;
type MsgHdr = [u8; MSGHDR];

pub(crate) fn net_err(e: NetError) -> i64 {
    -e.errno()
}

/// The socket handle of `fd` and its family and type.
fn socket_of<F, M, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
) -> Result<(Handle, Family, Type), i64> {
    let h = cx.fs.socket(fd as u32).map_err(err)?;
    let (family, ty) = cx.net.net().kind(h).map_err(net_err)?;
    Ok((h, family, ty))
}

/// A socket address as a cell gives it.
enum Given {
    Inet([u8; 4], u16),
    UnixPath(Vec<u8>),
}

/// Reads the socket address of `len` bytes at `addr` for a socket of
/// `family`.
fn read_addr<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    family: Family,
    addr: u64,
    len: u64,
) -> Result<Given, i64> {
    if !(2..=128).contains(&len) {
        return Err(-EINVAL);
    }
    let mut buf = [0u8; 128];
    let buf = &mut buf[..len as usize];
    cx.read(addr, buf).map_err(|_| -EFAULT)?;
    let given = u16::from_le_bytes([buf[0], buf[1]]);
    match family {
        Family::Inet => {
            if len < 16 {
                return Err(-EINVAL);
            }
            if given != AF_INET {
                return Err(-EAFNOSUPPORT);
            }
            let port = u16::from_be_bytes([buf[2], buf[3]]);
            Ok(Given::Inet([buf[4], buf[5], buf[6], buf[7]], port))
        }
        Family::Unix => {
            if given != AF_UNIX || len > 110 {
                return Err(-EINVAL);
            }
            let path = &buf[2..];
            let path = &path[..path.iter().position(|&b| b == 0).unwrap_or(path.len())];
            if path.is_empty() {
                return Err(-EINVAL); // abstract names and autobind: not supported
            }
            Ok(Given::UnixPath(path.to_vec()))
        }
    }
}

/// Writes `a` as a socket address at `addr`, its length at `lenp` (which
/// holds the room on entry).
fn write_addr<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    a: &Addr,
    addr: u64,
    lenp: u64,
) -> i64 {
    let mut bytes = Vec::with_capacity(16);
    match a {
        Addr::Inet { ip, port } => {
            bytes.extend_from_slice(&AF_INET.to_le_bytes());
            bytes.extend_from_slice(&port.to_be_bytes());
            bytes.extend_from_slice(ip);
            bytes.extend_from_slice(&[0; 8]);
        }
        Addr::Unix { path, .. } => {
            bytes.extend_from_slice(&AF_UNIX.to_le_bytes());
            bytes.extend_from_slice(path);
            bytes.push(0);
        }
        Addr::UnixUnnamed => bytes.extend_from_slice(&AF_UNIX.to_le_bytes()),
    }
    write_sized(cx, &bytes, addr, lenp)
}

/// Writes at most the room `*lenp` holds of `bytes` at `addr`, and their
/// full length at `lenp`.
fn write_sized<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    bytes: &[u8],
    addr: u64,
    lenp: u64,
) -> i64 {
    let mut room = [0u8; 4];
    if cx.read(lenp, &mut room).is_err() {
        return -EFAULT;
    }
    let room = i32::from_le_bytes(room);
    if room < 0 {
        return -EINVAL;
    }
    let n = (room as usize).min(bytes.len());
    if cx.write(addr, &bytes[..n]).is_err()
        || cx.write(lenp, &(bytes.len() as u32).to_le_bytes()).is_err()
    {
        return -EFAULT;
    }
    0
}

pub(crate) fn socket<F, M, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    domain: u64,
    ty: u64,
    protocol: u64,
) -> i64 {
    let flags = ty & (SOCK_NONBLOCK | SOCK_CLOEXEC);
    let family = match domain {
        1 => Family::Unix,
        2 => Family::Inet,
        _ => return -EAFNOSUPPORT,
    };
    let kind = match ty & !(SOCK_NONBLOCK | SOCK_CLOEXEC) {
        1 => Type::Stream,
        2 if family == Family::Inet => Type::Dgram,
        // A channel endpoint once connected (design D10 of
        // `add-cell-hierarchy`).
        5 if family == Family::Unix => Type::SeqPacket,
        _ => return -ESOCKTNOSUPPORT,
    };
    let protocol_ok = match (family, kind) {
        (Family::Inet, Type::Stream) => matches!(protocol, 0 | IPPROTO_TCP),
        (Family::Inet, Type::Dgram) => matches!(protocol, 0 | IPPROTO_UDP),
        (Family::Inet, Type::SeqPacket) => false,
        (Family::Unix, _) => protocol == 0,
    };
    if !protocol_ok {
        return -EPROTONOSUPPORT;
    }
    let h = match cx.net.net().socket(family, kind) {
        Ok(h) => h,
        Err(e) => return net_err(e),
    };
    open_fd(cx, h, flags)
}

/// A descriptor for new socket `h` (released if none is free).
fn open_fd<F, M, T, A, S>(cx: &mut Context<'_, F, M, T, A, S>, h: Handle, flags: u64) -> i64 {
    match cx
        .fs
        .socket_fd(h, flags & SOCK_NONBLOCK != 0, flags & SOCK_CLOEXEC != 0)
    {
        Ok(fd) => fd as i64,
        Err(e) => {
            cx.net.net().release(h);
            err(e)
        }
    }
}

pub(crate) fn socketpair<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    a: [u64; 6],
) -> i64 {
    let flags = a[1] & (SOCK_NONBLOCK | SOCK_CLOEXEC);
    match (a[0], a[1] & !flags, a[2]) {
        (1, 1, 0) => {}
        (1, _, _) => return -ESOCKTNOSUPPORT,
        (2, _, _) => return -EOPNOTSUPP,
        _ => return -EAFNOSUPPORT,
    }
    let (x, y) = match cx.net.net().pair() {
        Ok(pair) => pair,
        Err(e) => return net_err(e),
    };
    let fx = open_fd(cx, x, flags);
    if fx < 0 {
        cx.net.net().release(y);
        return fx;
    }
    let fy = open_fd(cx, y, flags);
    if fy < 0 {
        let _ = cx.fs.close(fx as u32);
        return fy;
    }
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&(fx as i32).to_le_bytes());
    out[4..].copy_from_slice(&(fy as i32).to_le_bytes());
    if cx.write(a[3], &out).is_err() {
        let _ = cx.fs.close(fx as u32);
        let _ = cx.fs.close(fy as u32);
        return -EFAULT;
    }
    0
}

pub(crate) fn bind<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    addr: u64,
    len: u64,
) -> i64 {
    let run = |cx: &mut Context<'_, F, M, T, A, S>| -> Result<i64, i64> {
        let (h, family, _) = socket_of(cx, fd)?;
        match read_addr(cx, family, addr, len)? {
            Given::Inet(ip, port) => cx
                .net
                .net()
                .bind(h, &Addr::Inet { ip, port })
                .map_err(net_err)?,
            Given::UnixPath(path) => {
                if cx.net.net().local_addr(h).map_err(net_err)?.is_some() {
                    return Err(-EINVAL);
                }
                let name = cx.net.net().new_name();
                cx.fs.bind_name(None, &path, name).map_err(|e| match e {
                    rhizo_fs::Error::Exists => -EADDRINUSE,
                    e => err(e),
                })?;
                if let Err(e) = cx.net.net().bind(
                    h,
                    &Addr::Unix {
                        name,
                        path: path.clone(),
                    },
                ) {
                    let _ = cx.fs.unlink(None, &path, false);
                    return Err(net_err(e));
                }
            }
        }
        Ok(0)
    };
    run(cx).unwrap_or_else(|e| e)
}

pub(crate) fn listen<F, M, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    backlog: u64,
) -> i64 {
    let (h, _, _) = match socket_of(cx, fd) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let backlog = match backlog as i32 {
        b if b < 0 => rhizo_net::BACKLOG_MAX,
        b => b as usize,
    };
    match cx.net.net().listen(h, backlog) {
        Ok(()) => 0,
        Err(e) => net_err(e),
    }
}

pub(crate) fn accept<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    addr: u64,
    lenp: u64,
    flags: u64,
) -> i64 {
    if flags & !(SOCK_NONBLOCK | SOCK_CLOEXEC) != 0 {
        return -EINVAL;
    }
    let (h, _, ty) = match socket_of(cx, fd) {
        Ok(s) => s,
        Err(e) => return e,
    };
    if ty == Type::Dgram {
        return -EOPNOTSUPP;
    }
    let conn = match cx.net.net().accept(h) {
        Ok(conn) => conn,
        Err(NetError::Again) => {
            let nonblocking = cx.fs.nonblocking(fd as u32).unwrap_or(true);
            let timeout = cx.net.net().options(h).map_or(0, |o| o.rcv_timeout_ns);
            return would_wait(cx, nonblocking, (timeout != 0).then_some(timeout));
        }
        Err(e) => return net_err(e),
    };
    let new = open_fd(cx, conn, flags);
    if new < 0 || addr == 0 {
        return new;
    }
    let peer = cx.net.net().peer_addr(conn).unwrap_or(Addr::UnixUnnamed);
    match write_addr(cx, &peer, addr, lenp) {
        0 => new,
        e => {
            let _ = cx.fs.close(new as u32);
            e
        }
    }
}

pub(crate) fn connect<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    addr: u64,
    len: u64,
) -> i64 {
    let run = |cx: &mut Context<'_, F, M, T, A, S>| -> Result<i64, i64> {
        let (h, family, ty) = socket_of(cx, fd)?;
        let to = match read_addr(cx, family, addr, len)? {
            Given::Inet(ip, port) => Addr::Inet { ip, port },
            Given::UnixPath(path) if ty == Type::SeqPacket => {
                return Ok(crate::packets::connect(cx, fd, h, &path));
            }
            Given::UnixPath(path) => {
                let name = cx.fs.socket_name(None, &path).map_err(err)?;
                Addr::Unix { name, path }
            }
        };
        let nonblocking = cx.fs.nonblocking(fd as u32).unwrap_or(false);
        match cx.net.net().connect(h, &to) {
            Ok(()) => {}
            // Through the network card the handshake takes time: a blocking
            // call waits and is made again until it reports the connection
            // (design D2 of `add-virtio-net`).
            Err(e @ (NetError::InProgress | NetError::Again)) if nonblocking => {
                return Err(if e == NetError::Again {
                    -EALREADY
                } else {
                    -EINPROGRESS
                });
            }
            Err(NetError::InProgress | NetError::Again) => {
                return Ok(block(cx, None).unwrap_or(-ETIMEDOUT));
            }
            Err(e) => return Err(net_err(e)),
        }
        // Over loopback the connection is made at once; Linux still answers
        // a non-blocking TCP connect with `EINPROGRESS` (design D6 of
        // `add-sockets-and-poll`).
        let loopback = matches!(to, Addr::Inet { ip, .. } if ip[0] == 127 || ip == rhizo_net::ANY);
        let pending = loopback && family == Family::Inet && ty == Type::Stream && nonblocking;
        Ok(if pending { -EINPROGRESS } else { 0 })
    };
    run(cx).unwrap_or_else(|e| e)
}

const EALREADY: i64 = 114;
const ETIMEDOUT: i64 = 110;

/// `write` on a socket, and `send`/`sendto` without a destination.
pub(crate) fn send<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u32,
    addr: u64,
    len: u64,
    flags: u64,
) -> i64 {
    sendto(cx, [u64::from(fd), addr, len, flags, 0, 0])
}

/// `sendto(fd, buf, len, flags, dest, dest_len)`.
pub(crate) fn sendto<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    a: [u64; 6],
) -> i64 {
    let (h, family, ty) = match socket_of(cx, a[0]) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let dest = match (ty, a[4]) {
        (Type::Dgram, addr) if addr != 0 => match read_addr(cx, family, addr, a[5]) {
            Ok(Given::Inet(ip, port)) => Some(Addr::Inet { ip, port }),
            Ok(Given::UnixPath(_)) => return -EINVAL,
            Err(e) => return e,
        },
        _ => None,
    };
    let cap = match ty {
        Type::Dgram if a[2] > rhizo_net::DGRAM_MAX as u64 => return -EMSGSIZE,
        Type::Dgram => a[2],
        Type::Stream => a[2].min(rhizo_net::STREAM_BUF as u64),
        Type::SeqPacket => return crate::packets::send(cx, a[0], h, a[1], a[2], a[3]),
    };
    with_user_bytes(cx, a[1], cap as usize, |cx, bytes| {
        send_bytes(cx, a[0], h, bytes, dest.as_ref(), a[3])
    })
}

const EMSGSIZE: i64 = 90;

/// Sends `bytes` (already copied from the cell) on socket `h` of `fd`.
fn send_bytes<F, M, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    h: Handle,
    bytes: &[u8],
    dest: Option<&Addr>,
    flags: u64,
) -> i64 {
    if !cx.services.can_store(bytes.len() as u64) {
        return -ENOBUFS;
    }
    match cx.net.net().send(h, bytes, dest) {
        Ok(n) => n as i64,
        Err(NetError::Again) => {
            let nonblocking =
                flags & MSG_DONTWAIT != 0 || cx.fs.nonblocking(fd as u32).unwrap_or(true);
            let timeout = cx.net.net().options(h).map_or(0, |o| o.snd_timeout_ns);
            would_wait(cx, nonblocking, (timeout != 0).then_some(timeout))
        }
        Err(e) => net_err(e),
    }
}

/// Where received bytes go: a list of cell buffers filled in order.
pub(crate) struct Scatter {
    iov: Vec<(u64, u64)>,
    at: usize,
    off: u64,
    pub(crate) faulted: bool,
}

impl Scatter {
    pub(crate) fn total(&self) -> u64 {
        // Each length is at most `SSIZE_MAX`; their sum may pass 2^64
        // (finding S16).
        self.iov
            .iter()
            .fold(0, |total: u64, &(_, len)| total.saturating_add(len))
    }

    /// Copies `piece` into the buffers; `false` (nothing counted) on a
    /// fault.
    pub(crate) fn put<F: EntryFormat, M: PhysicalMemory>(
        &mut self,
        pt: &PageTable<F>,
        mem: &mut M,
        pager: &mut dyn Pager<F, M>,
        mut piece: &[u8],
    ) -> bool {
        while !piece.is_empty() {
            let Some(&(base, len)) = self.iov.get(self.at) else {
                return true;
            };
            let n = (len - self.off).min(piece.len() as u64) as usize;
            if memory::write(pt, mem, pager, base.wrapping_add(self.off), &piece[..n]).is_err() {
                self.faulted = true;
                return false;
            }
            piece = &piece[n..];
            self.off += n as u64;
            if self.off == len {
                self.at += 1;
                self.off = 0;
            }
        }
        true
    }
}

/// Receives into `into` from socket `fd`; returns the result and, for a
/// datagram, its sender and whether it was cut.
fn recv_into<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    into: &mut Scatter,
    flags: u64,
) -> (i64, Option<Addr>, bool) {
    let (h, _, ty) = match socket_of(cx, fd) {
        Ok(s) => s,
        Err(e) => return (e, None, false),
    };
    let len = into.total();
    match ty {
        Type::Stream if len == 0 => return (0, None, false),
        Type::SeqPacket => {
            let (n, cut) = crate::packets::recv(cx, fd, h, into, flags);
            return (n, None, cut);
        }
        _ => {}
    }
    let (pt, mem, pager) = (&cx.pt, &mut *cx.mem, &mut *cx.pager);
    let got = cx
        .net
        .net()
        .recv(h, len as usize, flags & MSG_PEEK != 0, &mut |piece| {
            into.put(pt, mem, pager, piece)
        });
    match got {
        Ok(r) if r.len == 0 && into.faulted => (-EFAULT, None, false),
        Ok(r) => {
            let cut = r.full > r.len;
            let n = if flags & MSG_TRUNC != 0 && ty == Type::Dgram {
                r.full
            } else {
                r.len
            };
            (n as i64, r.from, cut)
        }
        Err(NetError::Again) => {
            let nonblocking =
                flags & MSG_DONTWAIT != 0 || cx.fs.nonblocking(fd as u32).unwrap_or(true);
            let timeout = cx.net.net().options(h).map_or(0, |o| o.rcv_timeout_ns);
            (
                would_wait(cx, nonblocking, (timeout != 0).then_some(timeout)),
                None,
                false,
            )
        }
        Err(e) => (net_err(e), None, false),
    }
}

/// `read` on a socket.
pub(crate) fn recv<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u32,
    addr: u64,
    len: u64,
    flags: u64,
) -> i64 {
    recvfrom(cx, [u64::from(fd), addr, len, flags, 0, 0])
}

/// `recvfrom(fd, buf, len, flags, src, src_len)`.
pub(crate) fn recvfrom<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    a: [u64; 6],
) -> i64 {
    let mut into = Scatter {
        iov: vec![(a[1], a[2])],
        at: 0,
        off: 0,
        faulted: false,
    };
    let (n, from, _) = recv_into(cx, a[0], &mut into, a[3]);
    if n < 0 || a[4] == 0 {
        return n;
    }
    let written = match from {
        Some(from) => write_addr(cx, &from, a[4], a[5]),
        // A stream fills no address.
        None => put(cx, a[5], &0u32.to_le_bytes()),
    };
    if written < 0 { written } else { n }
}

/// The parts of a `struct msghdr`: name, name length, buffers.
fn read_msghdr<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    msg: u64,
) -> Result<(MsgHdr, Vec<(u64, u64)>), i64> {
    let mut h = [0u8; MSGHDR];
    cx.read(msg, &mut h).map_err(|_| -EFAULT)?;
    let word = |at: usize| u64::from_le_bytes(h[at..at + 8].try_into().unwrap_or_default());
    let (iov, count) = (word(16), word(24));
    if count > 1024 {
        return Err(-EMSGSIZE);
    }
    let mut list = Vec::with_capacity(count as usize);
    for i in 0..count {
        let mut e = [0u8; 16];
        cx.read(iov.wrapping_add(i * 16), &mut e)
            .map_err(|_| -EFAULT)?;
        let base = u64::from_le_bytes(e[..8].try_into().unwrap_or_default());
        let len = u64::from_le_bytes(e[8..].try_into().unwrap_or_default());
        if len > i64::MAX as u64 {
            return Err(-EINVAL);
        }
        list.push((base, len));
    }
    Ok((h, list))
}

pub(crate) fn sendmsg<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    cell: u32,
    fd: u64,
    msg: u64,
    flags: u64,
) -> i64 {
    let run = |cx: &mut Context<'_, F, M, T, A, S>| -> Result<i64, i64> {
        // The descriptor first, as Linux checks it.
        cx.fs.socket(fd as u32).map_err(err)?;
        let (hdr, iov) = read_msghdr(cx, msg)?;
        let word = |at: usize| u64::from_le_bytes(hdr[at..at + 8].try_into().unwrap_or_default());
        // Handles go over channel endpoints only (design D9 of
        // `add-cell-hierarchy`); other sockets ignore control data.
        let rights = match word(40) {
            0 => Ok(Vec::new()),
            len => crate::packets::read_rights(cx, word(32), len),
        };
        if let Ok(fds) = &rights
            && !fds.is_empty()
            && let Some(r) = crate::packets::sendmsg_rights(cx, cell, fd, &iov, fds, flags)
        {
            return Ok(r);
        }
        let (h, family, ty) = socket_of(cx, fd)?;
        if ty == Type::SeqPacket {
            // No grants to move them with: no handle is sent.
            if !rights?.is_empty() {
                return Err(-EPERM);
            }
        }
        let name = u64::from_le_bytes(hdr[..8].try_into().unwrap_or_default());
        let name_len = u64::from(u32::from_le_bytes(
            hdr[8..12].try_into().unwrap_or_default(),
        ));
        let dest = match (ty, name) {
            (Type::Dgram, name) if name != 0 => match read_addr(cx, family, name, name_len)? {
                Given::Inet(ip, port) => Some(Addr::Inet { ip, port }),
                Given::UnixPath(_) => return Err(-EINVAL),
            },
            _ => None,
        };
        // Each length is at most `SSIZE_MAX`; their sum may pass 2^64
        // (finding S16).
        let total = iov
            .iter()
            .fold(0, |total: u64, &(_, len)| total.saturating_add(len));
        let cap = match ty {
            Type::Dgram if total > rhizo_net::DGRAM_MAX as u64 => return Err(-EMSGSIZE),
            Type::Dgram => total,
            Type::Stream => total.min(rhizo_net::STREAM_BUF as u64),
            Type::SeqPacket if total > u64::from(rhizo_cap::limits::MESSAGE_MAX) => {
                return Err(-EMSGSIZE);
            }
            Type::SeqPacket => total,
        };
        let mut bytes = vec![0u8; cap as usize];
        let mut at = 0usize;
        for (base, len) in iov {
            let n = (len as usize).min(bytes.len() - at);
            cx.read(base, &mut bytes[at..at + n]).map_err(|_| -EFAULT)?;
            at += n;
        }
        if ty == Type::SeqPacket {
            return Ok(crate::packets::send_bytes(cx, fd, h, &bytes, flags));
        }
        Ok(send_bytes(cx, fd, h, &bytes, dest.as_ref(), flags))
    };
    run(cx).unwrap_or_else(|e| e)
}

pub(crate) fn recvmsg<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    cell: u32,
    fd: u64,
    msg: u64,
    flags: u64,
) -> i64 {
    let (hdr, iov) = match read_msghdr(cx, msg) {
        Ok(parts) => parts,
        Err(e) => return e,
    };
    let mut into = Scatter {
        iov,
        at: 0,
        off: 0,
        faulted: false,
    };
    // Room for at least one handle: a channel endpoint delivers its
    // handles there (design D9 of `add-cell-hierarchy`).
    let word = |at: usize| u64::from_le_bytes(hdr[at..at + 8].try_into().unwrap_or_default());
    if word(40) >= 20
        && let Some(r) = crate::packets::recvmsg_rights(
            cx,
            cell,
            fd,
            msg,
            (word(32), word(40)),
            &mut into,
            flags,
        )
    {
        return r;
    }
    let (n, from, cut) = recv_into(cx, fd, &mut into, flags);
    if n < 0 {
        return n;
    }
    let name = u64::from_le_bytes(hdr[..8].try_into().unwrap_or_default());
    let written = match from {
        Some(from) if name != 0 => write_addr(cx, &from, name, msg + 8),
        _ => put(cx, msg + 8, &0u32.to_le_bytes()),
    };
    if written < 0 {
        return written;
    }
    // No control data; the flags say whether a datagram was cut.
    let msg_flags: u32 = if cut { MSG_TRUNC as u32 } else { 0 };
    if put(cx, msg + 40, &0u64.to_le_bytes()) < 0 || put(cx, msg + 48, &msg_flags.to_le_bytes()) < 0
    {
        return -EFAULT;
    }
    n
}

pub(crate) fn shutdown<F, M, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    how: u64,
) -> i64 {
    let how = match how {
        0 => Shutdown::Read,
        1 => Shutdown::Write,
        2 => Shutdown::Both,
        _ => return -EINVAL,
    };
    let (h, _, _) = match socket_of(cx, fd) {
        Ok(s) => s,
        Err(e) => return e,
    };
    match cx.net.net().shutdown(h, how) {
        Ok(()) => 0,
        Err(e) => net_err(e),
    }
}

/// `getsockname` (`peer` false) and `getpeername` (`peer` true).
pub(crate) fn name<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    addr: u64,
    lenp: u64,
    peer: bool,
) -> i64 {
    let (h, family, _) = match socket_of(cx, fd) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let a = if peer {
        match cx.net.net().peer_addr(h) {
            Ok(a) => a,
            Err(e) => return net_err(e),
        }
    } else {
        match (cx.net.net().local_addr(h), family) {
            (Ok(Some(a)), _) => a,
            (Ok(None), Family::Inet) => Addr::Inet {
                ip: rhizo_net::ANY,
                port: 0,
            },
            (Ok(None), Family::Unix) => Addr::UnixUnnamed,
            (Err(e), _) => return net_err(e),
        }
    };
    write_addr(cx, &a, addr, lenp)
}

/// Socket options (`SO_*`, `TCP_*`).
mod opt {
    pub const SO_DEBUG: u64 = 1;
    pub const SO_REUSEADDR: u64 = 2;
    pub const SO_TYPE: u64 = 3;
    pub const SO_ERROR: u64 = 4;
    pub const SO_DONTROUTE: u64 = 5;
    pub const SO_BROADCAST: u64 = 6;
    pub const SO_SNDBUF: u64 = 7;
    pub const SO_RCVBUF: u64 = 8;
    pub const SO_KEEPALIVE: u64 = 9;
    pub const SO_OOBINLINE: u64 = 10;
    pub const SO_PRIORITY: u64 = 12;
    pub const SO_LINGER: u64 = 13;
    pub const SO_REUSEPORT: u64 = 15;
    pub const SO_RCVLOWAT: u64 = 18;
    pub const SO_RCVTIMEO: u64 = 20;
    pub const SO_SNDTIMEO: u64 = 21;
    pub const SO_ACCEPTCONN: u64 = 30;
    pub const SO_PROTOCOL: u64 = 38;
    pub const SO_DOMAIN: u64 = 39;
    pub const SO_RCVTIMEO_NEW: u64 = 66;
    pub const SO_SNDTIMEO_NEW: u64 = 67;
    pub const TCP_NODELAY: u64 = 1;
    pub const TCP_KEEPIDLE: u64 = 4;
    pub const TCP_KEEPINTVL: u64 = 5;
    pub const TCP_KEEPCNT: u64 = 6;
    pub const IP_TOS: u64 = 1;
    pub const IP_TTL: u64 = 2;
}
use opt::*;

pub(crate) fn setsockopt<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    a: [u64; 6],
) -> i64 {
    let run = |cx: &mut Context<'_, F, M, T, A, S>| -> Result<i64, i64> {
        let (h, family, ty) = socket_of(cx, a[0])?;
        let (level, name, val, len) = (a[1], a[2], a[3], a[4] as u32 as usize);
        let mut raw = [0u8; 16];
        let want = match name {
            SO_LINGER if level == SOL_SOCKET => 8,
            SO_RCVTIMEO | SO_SNDTIMEO | SO_RCVTIMEO_NEW | SO_SNDTIMEO_NEW
                if level == SOL_SOCKET =>
            {
                16
            }
            _ => 4,
        };
        if len < want {
            return Err(-EINVAL);
        }
        cx.read(val, &mut raw[..want]).map_err(|_| -EFAULT)?;
        let int = i32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
        let mut o = cx.net.net().options(h).map_err(net_err)?;
        let buf = |v: i32, min: u32| (v.max(0) as u32).min(BUF_MAX).saturating_mul(2).max(min);
        let tcp = family == Family::Inet && ty == Type::Stream;
        match (level, name) {
            (SOL_SOCKET, SO_REUSEADDR) => o.reuse_addr = int != 0,
            (SOL_SOCKET, SO_REUSEPORT) => o.reuse_port = int != 0,
            (SOL_SOCKET, SO_KEEPALIVE) => o.keepalive = int != 0,
            (SOL_SOCKET, SO_BROADCAST) => o.broadcast = int != 0,
            (SOL_SOCKET, SO_SNDBUF) => o.sndbuf = buf(int, 4608),
            (SOL_SOCKET, SO_RCVBUF) => o.rcvbuf = buf(int, 2304),
            (SOL_SOCKET, SO_LINGER) => {
                let secs = i32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
                o.linger = (int != 0).then_some(secs.max(0) as u32);
            }
            (SOL_SOCKET, SO_RCVTIMEO | SO_SNDTIMEO | SO_RCVTIMEO_NEW | SO_SNDTIMEO_NEW) => {
                let sec = i64::from_le_bytes(raw[..8].try_into().unwrap_or_default());
                let usec = i64::from_le_bytes(raw[8..].try_into().unwrap_or_default());
                if !(0..1_000_000).contains(&usec) {
                    return Err(-33); // EDOM
                }
                let ns = (sec.max(0) as u64)
                    .saturating_mul(1_000_000_000)
                    .saturating_add(usec as u64 * 1000);
                if matches!(name, SO_RCVTIMEO | SO_RCVTIMEO_NEW) {
                    o.rcv_timeout_ns = ns;
                } else {
                    o.snd_timeout_ns = ns;
                }
            }
            // Accepted, with no effect on loopback.
            (SOL_SOCKET, SO_DEBUG | SO_DONTROUTE | SO_OOBINLINE | SO_PRIORITY | SO_RCVLOWAT) => {}
            (IPPROTO_IP, IP_TOS | IP_TTL) if family == Family::Inet => {}
            (IPPROTO_TCP, _) if !tcp => return Err(-EOPNOTSUPP),
            (IPPROTO_TCP, TCP_NODELAY) => o.no_delay = int != 0,
            (IPPROTO_TCP, TCP_KEEPIDLE | TCP_KEEPINTVL) if !(1..=32767).contains(&int) => {
                return Err(-EINVAL);
            }
            (IPPROTO_TCP, TCP_KEEPIDLE) => o.keep_idle = int as u32,
            (IPPROTO_TCP, TCP_KEEPINTVL) => o.keep_intvl = int as u32,
            (IPPROTO_TCP, TCP_KEEPCNT) if !(1..=127).contains(&int) => return Err(-EINVAL),
            (IPPROTO_TCP, TCP_KEEPCNT) => o.keep_cnt = int as u32,
            _ => return Err(-ENOPROTOOPT),
        }
        cx.net.net().set_options(h, o).map_err(net_err)?;
        Ok(0)
    };
    run(cx).unwrap_or_else(|e| e)
}

pub(crate) fn getsockopt<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    a: [u64; 6],
) -> i64 {
    let (h, family, ty) = match socket_of(cx, a[0]) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let Ok(o) = cx.net.net().options(h) else {
        return -EBADF;
    };
    let (level, name) = (a[1], a[2]);
    let int = |v: u32| v.to_le_bytes().to_vec();
    let flag = |b: bool| int(b as u32);
    let time = |ns: u64| {
        let mut v = (ns / 1_000_000_000).to_le_bytes().to_vec();
        v.extend_from_slice(&(ns % 1_000_000_000 / 1000).to_le_bytes());
        v
    };
    let tcp = family == Family::Inet && ty == Type::Stream;
    let bytes = match (level, name) {
        (SOL_SOCKET, SO_REUSEADDR) => flag(o.reuse_addr),
        (SOL_SOCKET, SO_REUSEPORT) => flag(o.reuse_port),
        (SOL_SOCKET, SO_KEEPALIVE) => flag(o.keepalive),
        (SOL_SOCKET, SO_BROADCAST) => flag(o.broadcast),
        (SOL_SOCKET, SO_SNDBUF) => int(o.sndbuf),
        (SOL_SOCKET, SO_RCVBUF) => int(o.rcvbuf),
        (SOL_SOCKET, SO_LINGER) => {
            let mut v = int(o.linger.is_some() as u32);
            v.extend_from_slice(&o.linger.unwrap_or(0).to_le_bytes());
            v
        }
        (SOL_SOCKET, SO_RCVTIMEO | SO_RCVTIMEO_NEW) => time(o.rcv_timeout_ns),
        (SOL_SOCKET, SO_SNDTIMEO | SO_SNDTIMEO_NEW) => time(o.snd_timeout_ns),
        (SOL_SOCKET, SO_TYPE) => int(match ty {
            Type::Stream => 1,
            Type::Dgram => 2,
            Type::SeqPacket => 5,
        }),
        (SOL_SOCKET, SO_DOMAIN) => int(if family == Family::Inet { 2 } else { 1 }),
        (SOL_SOCKET, SO_PROTOCOL) => int(match (family, ty) {
            (Family::Inet, Type::Stream) => IPPROTO_TCP as u32,
            (Family::Inet, Type::Dgram) => IPPROTO_UDP as u32,
            _ => 0,
        }),
        (SOL_SOCKET, SO_ACCEPTCONN) => flag(cx.net.net().is_listening(h)),
        (SOL_SOCKET, SO_ERROR) => {
            let e = cx.net.net().take_error(h).ok().flatten();
            int(e.map_or(0, |e| e.errno() as u32))
        }
        (IPPROTO_TCP, _) if !tcp => return -EOPNOTSUPP,
        (IPPROTO_TCP, TCP_NODELAY) => flag(o.no_delay),
        (IPPROTO_TCP, TCP_KEEPIDLE) => int(o.keep_idle),
        (IPPROTO_TCP, TCP_KEEPINTVL) => int(o.keep_intvl),
        (IPPROTO_TCP, TCP_KEEPCNT) => int(o.keep_cnt),
        _ => return -ENOPROTOOPT,
    };
    write_sized(cx, &bytes, a[3], a[4])
}
