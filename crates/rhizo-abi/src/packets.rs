//! Channel endpoints as connected `AF_UNIX` `SOCK_SEQPACKET` sockets
//! (spec `cell-hierarchy`: channels as descriptors, transfer over
//! channels; design D9 and D10 of `add-cell-hierarchy`).
//!
//! An endpoint is a socket of the kernel's [`rhizo_net::Net`], so a
//! descriptor of one is polled, waited on, duplicated and closed as any
//! socket's. Each `write` sends one message and each `read` takes one, cut
//! to the buffer (`MSG_TRUNC`). `connect` to, or `open` of,
//! `/run/rhizo/chan/<label>` gives a descriptor of the endpoint the entry
//! holds, and several share its queue.
//!
//! Handles move as `SCM_RIGHTS`, and only handles: a descriptor of a
//! file, pipe, socket, `epoll` instance or event counter is refused with
//! `EPERM`. The receiver gets a grant derived from the sender's, with the
//! sender as grantor ([`rhizo_cap::Grants::transfer`]), made when the
//! message is sent for the holder of the peer endpoint and checked again
//! for another holder that takes it ([`rhizo_cap::Grants::rehome`]).

use super::*;
use crate::files::err;
use crate::poll::{with_user_bytes, would_wait};
use crate::sockets::{MSG_DONTWAIT, MSG_PEEK, MSG_TRUNC, Scatter, net_err};
use alloc::vec::Vec;
use rhizo_cap::limits::{MESSAGE_HANDLES_MAX, MESSAGE_MAX};
use rhizo_cap::{Actor, Ask, Object, Recv};
use rhizo_fs::Kept;
use rhizo_net::{Error as NetError, Family, Handle, Type};

const H: usize = MESSAGE_HANDLES_MAX as usize;
const SOL_SOCKET: u32 = 1;
const SCM_RIGHTS: u32 = 1;
/// `struct cmsghdr`: `cmsg_len` (8 bytes), `cmsg_level`, `cmsg_type`.
const CMSG_HDR: u64 = 16;
/// Most control bytes a `sendmsg` may give (more fail with `ENOBUFS`).
const CONTROL_MAX: u64 = 1024;
const MSG_CTRUNC: u32 = 0x8;
const MSG_CMSG_CLOEXEC: u64 = 0x4000_0000;
const ENXIO: i64 = 6;
const EMLINK: i64 = 31;
const EMSGSIZE: i64 = 90;
const ETOOMANYREFS: i64 = 109;
const EISCONN: i64 = 106;
const ECONNREFUSED: i64 = 111;

/// Whether `h` is a sequenced-packet socket: an endpoint, one that reads
/// as closed, or one not connected yet.
pub(crate) fn is_packet(net: &rhizo_net::Net, h: Handle) -> bool {
    net.kind(h) == Ok((Family::Unix, Type::SeqPacket))
}

/// What a call on endpoint socket `h` of `fd` that would wait returns:
/// `EAGAIN` when non-blocking (or `MSG_DONTWAIT`) or past its timeout,
/// else it blocks until the endpoint changes.
fn wait<F, M, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    h: Handle,
    flags: u64,
    send: bool,
) -> i64 {
    let nonblocking = flags & MSG_DONTWAIT != 0 || cx.fs.nonblocking(fd as u32).unwrap_or(true);
    let timeout = cx.net.net().options(h).map_or(0, |o| {
        if send {
            o.snd_timeout_ns
        } else {
            o.rcv_timeout_ns
        }
    });
    would_wait(cx, nonblocking, (timeout != 0).then_some(timeout))
}

/// `write`, `send` and `sendto` on endpoint socket `h` of `fd`: one
/// message of the `len` bytes at `addr`.
pub(crate) fn send<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    h: Handle,
    addr: u64,
    len: u64,
    flags: u64,
) -> i64 {
    if len > u64::from(MESSAGE_MAX) {
        return -EMSGSIZE;
    }
    with_user_bytes(cx, addr, len as usize, |cx, bytes| {
        send_bytes(cx, fd, h, bytes, flags)
    })
}

/// Sends `bytes`, one message with no handle, on endpoint socket `h`.
pub(crate) fn send_bytes<F, M, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    h: Handle,
    bytes: &[u8],
    flags: u64,
) -> i64 {
    match cx.net.net().send_message(h, bytes, &[]) {
        Ok(()) => bytes.len() as i64,
        Err(NetError::Again) => wait(cx, fd, h, flags, true),
        Err(e) => net_err(e),
    }
}

/// `read`, `recv`, `recvfrom` and a `recvmsg` with no room for handles on
/// endpoint socket `h` of `fd`: the next message into `into`, its handles
/// let go. Returns the result and whether the message was cut.
pub(crate) fn recv<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    h: Handle,
    into: &mut Scatter,
    flags: u64,
) -> (i64, bool) {
    if flags & MSG_PEEK != 0 {
        return (-EOPNOTSUPP, false);
    }
    let mut ids = [0; H];
    let (pt, mem, pager) = (&cx.pt, &mut *cx.mem, &mut *cx.pager);
    let net = cx.net.net();
    let max = into.total().min(usize::MAX as u64) as usize;
    let got = net.recv_message(h, max, &mut ids, &mut |piece| {
        into.put(pt, mem, pager, piece)
    });
    match got {
        Ok(Recv::Message { len, handles }) => {
            for &id in &ids[..handles] {
                net.release(id);
            }
            cx.services.received();
            message_result(into, len, flags)
        }
        Ok(_) => (0, false),
        Err(NetError::Again) => (wait(cx, fd, h, flags, false), false),
        Err(e) => (net_err(e), false),
    }
}

/// What a receive of a `len`-byte message into `into` returns: the bytes
/// copied (the full length with `MSG_TRUNC`), and whether it was cut.
fn message_result(into: &Scatter, len: usize, flags: u64) -> (i64, bool) {
    let copied = (len as u64).min(into.total());
    if into.faulted && copied > 0 {
        return (-EFAULT, false);
    }
    let n = if flags & MSG_TRUNC != 0 {
        len as u64
    } else {
        copied
    };
    (n as i64, copied < len as u64)
}

/// The descriptors an `SCM_RIGHTS` control message of `sendmsg` names:
/// `control` and `len` from its `msghdr`. Other control messages fail
/// with `EINVAL`.
pub(crate) fn read_rights<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    control: u64,
    len: u64,
) -> Result<Vec<u32>, i64> {
    let mut fds = Vec::new();
    if len < CMSG_HDR {
        return Ok(fds);
    }
    if len > CONTROL_MAX {
        return Err(-ENOBUFS);
    }
    let mut buf = [0u8; CONTROL_MAX as usize];
    let buf = &mut buf[..len as usize];
    cx.read(control, buf).map_err(|_| -EFAULT)?;
    let mut at = 0usize;
    while at + CMSG_HDR as usize <= buf.len() {
        let word = |o: usize| u32::from_le_bytes(buf[o..o + 4].try_into().unwrap_or_default());
        let cmsg_len = u64::from_le_bytes(buf[at..at + 8].try_into().unwrap_or_default());
        if cmsg_len < CMSG_HDR || cmsg_len > (buf.len() - at) as u64 {
            return Err(-EINVAL);
        }
        if (word(at + 8), word(at + 12)) != (SOL_SOCKET, SCM_RIGHTS) {
            return Err(-EINVAL);
        }
        let data = at + CMSG_HDR as usize;
        for i in 0..(cmsg_len - CMSG_HDR) as usize / 4 {
            fds.push(word(data + 4 * i));
        }
        at += (cmsg_len as usize).next_multiple_of(8);
    }
    Ok(fds)
}

/// What a transfer refused by the grant table returns.
fn grant_err(e: rhizo_cap::Error) -> i64 {
    match e {
        rhizo_cap::Error::Denied => -EACCES,
        rhizo_cap::Error::TooManyHandles => -ETOOMANYREFS,
        rhizo_cap::Error::TooDeep => -EMLINK,
        rhizo_cap::Error::NoMemory => -ENOMEM,
        _ => -EINVAL,
    }
}

/// What a `sendmsg` with handles does once checked.
enum Sent {
    Done(i64),
    Wait,
}

/// `sendmsg` with handles (`fds`, from [`read_rights`]) of the cell `cell`
/// on descriptor `fd`, the message's bytes in the buffers `iov`. `None` if
/// `fd` is no sequenced-packet socket, or the kernel moves no handles:
/// the caller goes on without them.
pub(crate) fn sendmsg_rights<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    cell: u32,
    fd: u64,
    iov: &[(u64, u64)],
    fds: &[u32],
    flags: u64,
) -> Option<i64> {
    let h = cx.fs.socket(fd as u32).ok()?;
    let total = iov
        .iter()
        .fold(0, |total: u64, &(_, len)| total.saturating_add(len));
    let sent = {
        let hs = cx.net.handles()?;
        if !is_packet(hs.net, h) {
            return None;
        }
        if total > u64::from(MESSAGE_MAX) {
            return Some(-EMSGSIZE);
        }
        let mut bytes = alloc::vec![0u8; total as usize];
        let mut at = 0usize;
        for &(base, len) in iov {
            let n = len as usize;
            if memory::read(&cx.pt, cx.mem, cx.pager, base, &mut bytes[at..at + n]).is_err() {
                return Some(-EFAULT);
            }
            at += n;
        }
        let Some((end, over, rights)) = hs.net.endpoint_of(h) else {
            return Some(-EPIPE);
        };
        if fds.len() > H {
            return Some(-ETOOMANYREFS);
        }
        if !rights.contains(rhizo_cap::Rights::CARRY) {
            return Some(-EACCES);
        }
        // The grants go to a holder of the peer endpoint.
        let Some(to) = hs
            .net
            .end_grants(end ^ 1)
            .find_map(|g| hs.grants.get(g).map(rhizo_cap::Grant::holder))
        else {
            return Some(-EPIPE);
        };
        let mut moved: [(u32, Handle); H] = [(0, 0); H];
        let mut n = 0;
        let mut refused = None;
        for &f in fds {
            let what = cx
                .fs
                .socket(f)
                .map_err(|e| match e {
                    rhizo_fs::Error::BadFd => -EBADF,
                    _ => -EPERM,
                })
                .and_then(|s| hs.net.endpoint_of(s).ok_or(-EPERM));
            let (e, from, r) = match what {
                Ok(x) => x,
                Err(e) => {
                    refused = Some(e);
                    break;
                }
            };
            let ask = Ask {
                rights: r,
                holder: to,
                ..Ask::default()
            };
            let g = match hs.grants.transfer(hs.tree, cell, from, over, ask, &|_| {
                rhizo_cap::AccelBudget::ZERO
            }) {
                Ok(g) => g,
                Err(e) => {
                    refused = Some(grant_err(e));
                    break;
                }
            };
            match hs.net.endpoint(e, g, r) {
                Ok(s) => {
                    moved[n] = (g, s);
                    n += 1;
                }
                Err(e) => {
                    let _ = hs.grants.revoke(hs.tree, Actor::Host, g, &mut |_, _| {});
                    refused = Some(net_err(e));
                    break;
                }
            }
        }
        let socks: Vec<Handle> = moved[..n].iter().map(|&(_, s)| s).collect();
        let result = match refused {
            Some(e) => Err(e),
            None => hs.net.send_message(h, &bytes, &socks).map_err(|e| match e {
                NetError::Again => 0,
                e => net_err(e),
            }),
        };
        if result.is_err() {
            // Nothing was sent: the grants made for it go.
            for &(g, s) in &moved[..n] {
                hs.net.grant_gone(g);
                hs.net.release(s);
                let _ = hs.grants.revoke(hs.tree, Actor::Host, g, &mut |_, _| {});
            }
        }
        match result {
            Ok(()) => Sent::Done(total as i64),
            Err(0) => Sent::Wait,
            Err(e) => Sent::Done(e),
        }
    };
    Some(match sent {
        Sent::Done(r) => r,
        Sent::Wait => wait(cx, fd, h, flags, true),
    })
}

/// What a `recvmsg` with room for handles found.
enum Got {
    /// A message: its length, and the descriptors made for its handles
    /// (with whether some were let go for lack of room).
    Message(usize, Vec<u32>, bool),
    End,
    Wait,
    Failed(i64),
}

/// `recvmsg` of cell `cell` on descriptor `fd` with `control` bytes of
/// room at `at` for handles: the next message into `into`, each handle a
/// new descriptor named in an `SCM_RIGHTS` control message; writes the
/// control length and flags of the `msghdr` at `msg`. `None` if `fd` is
/// no sequenced-packet socket, or the kernel moves no handles: the caller
/// goes on.
pub(crate) fn recvmsg_rights<F: EntryFormat, M: PhysicalMemory, T, A: Frames, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    cell: u32,
    fd: u64,
    msg: u64,
    (at, control): (u64, u64),
    into: &mut Scatter,
    flags: u64,
) -> Option<i64> {
    let h = cx.fs.socket(fd as u32).ok()?;
    let got = {
        let hs = cx.net.handles()?;
        if !is_packet(hs.net, h) {
            return None;
        }
        if flags & MSG_PEEK != 0 {
            return Some(-EOPNOTSUPP);
        }
        let on = hs.net.endpoint_of(h).map(|(e, _, _)| Object::Endpoint(e));
        let mut ids = [0; H];
        let (pt, mem, pager) = (&cx.pt, &mut *cx.mem, &mut *cx.pager);
        let max = into.total().min(usize::MAX as u64) as usize;
        let got = hs.net.recv_message(h, max, &mut ids, &mut |piece| {
            into.put(pt, mem, pager, piece)
        });
        match got {
            Ok(Recv::Message { len, handles }) => {
                let room = ((control - CMSG_HDR) / 4) as usize;
                let cloexec = flags & MSG_CMSG_CLOEXEC != 0;
                let (mut fds, mut cut) = (Vec::new(), false);
                for &id in &ids[..handles] {
                    // A handle sent to another holder of this endpoint
                    // becomes this cell's, if the checks allow.
                    let mine = match (hs.net.endpoint_of(id), on) {
                        (Some((_, g, _)), Some(on))
                            if hs.grants.get(g).is_some_and(|x| x.holder() != cell) =>
                        {
                            let moved = hs.grants.rehome(hs.tree, g, cell, on);
                            if moved.is_err() {
                                hs.net.grant_gone(g);
                                let _ = hs.grants.revoke(hs.tree, Actor::Host, g, &mut |_, _| {});
                            }
                            moved.is_ok()
                        }
                        _ => true,
                    };
                    // Its descriptor counts as a socket's against the
                    // cell's memory (design D4 of `add-cell-hierarchy`).
                    let bytes = rhizo_net::SOCKET_BYTES;
                    let counted = mine
                        && fds.len() < room
                        && cx.frames.charge(bytes)
                        && (cx.services.can_store(bytes) || {
                            cx.frames.credit(bytes);
                            false
                        });
                    let new = match counted {
                        true => cx.fs.socket_fd(id, false, cloexec).ok(),
                        false => None,
                    };
                    if counted && new.is_none() {
                        cx.frames.credit(bytes);
                    }
                    match new {
                        Some(f) => fds.push(f),
                        None => {
                            hs.net.release(id);
                            cut = true;
                        }
                    }
                }
                Got::Message(len, fds, cut)
            }
            Ok(_) => Got::End,
            Err(NetError::Again) => Got::Wait,
            Err(e) => Got::Failed(net_err(e)),
        }
    };
    let (len, fds, ctrunc) = match got {
        Got::Message(len, fds, cut) => {
            cx.services.received();
            (len, fds, cut)
        }
        Got::End => (0, Vec::new(), false),
        Got::Wait => return Some(wait(cx, fd, h, flags, false)),
        Got::Failed(e) => return Some(e),
    };
    let (n, cut) = message_result(into, len, flags);
    let mut used = 0u64;
    let mut ok = n >= 0;
    if ok && !fds.is_empty() {
        let mut cmsg = Vec::with_capacity(CMSG_HDR as usize + 4 * fds.len());
        cmsg.extend_from_slice(&(CMSG_HDR + 4 * fds.len() as u64).to_le_bytes());
        cmsg.extend_from_slice(&SOL_SOCKET.to_le_bytes());
        cmsg.extend_from_slice(&SCM_RIGHTS.to_le_bytes());
        for f in &fds {
            cmsg.extend_from_slice(&f.to_le_bytes());
        }
        used = (cmsg.len() as u64).next_multiple_of(8).min(control);
        ok = cx.write(at, &cmsg).is_ok();
    }
    let msg_flags = if cut { MSG_TRUNC as u32 } else { 0 } | if ctrunc { MSG_CTRUNC } else { 0 };
    ok = ok
        && put(cx, msg + 8, &0u32.to_le_bytes()) == 0
        && put(cx, msg + 40, &used.to_le_bytes()) == 0
        && put(cx, msg + 48, &msg_flags.to_le_bytes()) == 0;
    if !ok {
        for f in fds {
            let _ = cx.fs.close(f);
        }
        return Some(if n < 0 { n } else { -EFAULT });
    }
    Some(n)
}

/// `connect` of sequenced-packet socket `h` of `fd` to `path`: a
/// `/run/rhizo` entry holding an endpoint makes `fd` a descriptor of it.
pub(crate) fn connect<F, M, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    h: Handle,
    path: &[u8],
) -> i64 {
    let net = cx.net.net();
    if net.peer_addr(h).is_ok() {
        return -EISCONN;
    }
    let e = match cx.fs.rhizo_entry(None, path) {
        Ok(Kept::Socket(e)) if is_packet(net, e) => e,
        Err(rhizo_fs::Error::NotFound) => return -ENOENT,
        Err(e @ (rhizo_fs::Error::NotDir | rhizo_fs::Error::NameTooLong)) => return err(e),
        _ => return -ECONNREFUSED,
    };
    net.retain(e);
    match cx.fs.replace_socket(fd as u32, e) {
        Ok(old) => {
            net.release(old);
            0
        }
        Err(x) => {
            net.release(e);
            err(x)
        }
    }
}

/// `open` of the `/run/rhizo` entry `kept`: a descriptor of the endpoint
/// it holds; `ENXIO` for any other entry.
pub(crate) fn open<F, M, T, A: Frames, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    kept: Kept,
    nonblock: bool,
    cloexec: bool,
) -> i64 {
    let net = cx.net.net();
    let Kept::Socket(e) = kept else {
        return -ENXIO;
    };
    if !is_packet(net, e) {
        return -ENXIO;
    }
    // Its descriptor counts as a socket's against the cell's memory
    // (design D4 of `add-cell-hierarchy`).
    let before = cx.held();
    if !cx.store(rhizo_net::SOCKET_BYTES) {
        return -ENOMEM;
    }
    cx.net.net().retain(e);
    let opened = cx.fs.socket_fd(e, nonblock, cloexec);
    cx.settle(rhizo_net::SOCKET_BYTES, before);
    match opened {
        Ok(fd) => fd.into(),
        Err(x) => {
            cx.net.net().release(e);
            err(x)
        }
    }
}
