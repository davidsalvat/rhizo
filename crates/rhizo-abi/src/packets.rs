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
use rhizo_cap::{Actor, Ask, Object, Recv, Rights};
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
    let reserved = cx.reserved();
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
                        && (reserved || cx.services.can_store(bytes) || {
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

/// `connect` of sequenced-packet socket `h` of `fd`, cell `cell`'s, to
/// `path`: a `/run/rhizo` entry holding an endpoint makes `fd` a
/// descriptor of it; a cell handle with the message right opens a new
/// channel to its cell ([`connect_handle`]).
pub(crate) fn connect<F, M, T, A: Frames, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    cell: u32,
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
        Ok(Kept::Grant(g)) => {
            let label = path.rsplit(|&b| b == b'/').next().unwrap_or_default();
            return match connect_handle(cx, cell, g, label) {
                Ok(e) => give(cx, fd, e),
                Err(x) => x,
            };
        }
        Err(rhizo_fs::Error::NotFound) => return -ENOENT,
        Err(e @ (rhizo_fs::Error::NotDir | rhizo_fs::Error::NameTooLong)) => return err(e),
        _ => return -ECONNREFUSED,
    };
    net.retain(e);
    give(cx, fd, e)
}

/// Makes `fd` a descriptor of socket `e`, whose reference it takes over.
fn give<F, M, T, A, S>(cx: &mut Context<'_, F, M, T, A, S>, fd: u64, e: Handle) -> i64 {
    match cx.fs.replace_socket(fd as u32, e) {
        Ok(old) => {
            cx.net.net().release(old);
            0
        }
        Err(x) => {
            cx.net.net().release(e);
            err(x)
        }
    }
}

/// The cell that cell `cell`'s grant `g`, its `/run/rhizo/cell/<label>`
/// entry, names, if it holds every right of `want`; `EACCES` otherwise: a
/// handle revoked, gone with its cell, inherited from the copy's source,
/// or short of a right (design D9 of `add-cell-hierarchy`).
fn target(
    grants: &rhizo_cap::Grants,
    cell: u32,
    g: u32,
    label: &[u8],
    want: Rights,
) -> Result<u32, i64> {
    let x = grants.get(g).ok_or(-EACCES)?;
    match x.object() {
        Object::Cell(t)
            if x.holder() == cell && x.label().as_bytes() == label && x.rights().contains(want) =>
        {
            Ok(t)
        }
        _ => Err(-EACCES),
    }
}

/// What a channel refused by the channels or the sockets returns.
fn channel_err(e: rhizo_cap::Error) -> i64 {
    match e {
        rhizo_cap::Error::Budget | rhizo_cap::Error::NoMemory => -ENOMEM,
        e => grant_err(e),
    }
}

/// Opening a channel to the cell of cell `cell`'s handle `g` (its entry
/// `cell/<label>`), which needs the message right (design D10 of
/// `add-cell-hierarchy`): a channel `cell` makes, its room counted against
/// `cell`'s memory until the channel ends, one endpoint for `cell` and one
/// waiting in the target's inbox, both sending and receiving. Returns the
/// socket of `cell`'s endpoint, with one reference.
fn connect_handle<F, M, T, A: Frames, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    cell: u32,
    g: u32,
    label: &[u8],
) -> Result<Handle, i64> {
    if !cx.store(rhizo_cap::ROOM) {
        return Err(-ENOMEM);
    }
    let mut created = false;
    let made = (|| {
        let hs = cx.net.handles().ok_or(-EACCES)?;
        let t = target(hs.grants, cell, g, label, Rights::MESSAGE)?;
        let ch = hs
            .net
            .channels_mut()
            .create(hs.tree, Actor::Cell(cell), &|_| rhizo_cap::Limits::ZERO)
            .map_err(channel_err)?;
        // From here the channel's end gives its room back.
        created = true;
        let (mine, theirs) = match hs
            .grants
            .open_channel(hs.tree, hs.net.channels(), cell, g, ch)
        {
            Ok(x) => x,
            Err(e) => {
                hs.net.drop_channel(ch);
                return Err(grant_err(e));
            }
        };
        let [e0, e1] = rhizo_cap::Channels::endpoints(ch);
        let both = Rights::SEND | Rights::RECEIVE;
        let s0 = hs.net.endpoint(e0, mine, both);
        let s1 = hs.net.endpoint(e1, theirs, both);
        let delivered = match (s0, s1) {
            (Ok(_), Ok(s1)) => hs.net.deliver(t, s1).map_err(net_err),
            (_, Err(e)) | (Err(e), _) => Err(net_err(e)),
        };
        if let Err(x) = delivered {
            for (id, s) in [(mine, s0), (theirs, s1)] {
                hs.net.grant_gone(id);
                if let Ok(s) = s {
                    hs.net.release(s);
                }
                let _ = hs.grants.revoke(hs.tree, Actor::Host, id, &mut |_, _| {});
            }
            hs.net.drop_channel(ch);
            return Err(x);
        }
        s0.map_err(net_err)
    })();
    if made.is_err() && !created {
        cx.frames.credit(rhizo_cap::ROOM);
    }
    made
}

/// Observing or giving input to the cell of cell `cell`'s handle `g` (its
/// entry `cell/<label>`), as `open` of the entry for reading, writing or
/// both (`access`) asks, which needs the observe right, the input right or
/// both (task 2.5 of `add-cell-hierarchy`): a channel `cell` makes, its
/// room counted against `cell`'s memory until it ends, whose other
/// endpoint the kernel holds for the target. Returns the socket of
/// `cell`'s endpoint, with one reference.
fn open_handle<F, M, T, A: Frames, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    cell: u32,
    g: u32,
    label: &[u8],
    access: u64,
) -> Result<Handle, i64> {
    let (observe, input) = (access != 1, access != 0);
    let pick = |on, r| if on { r } else { Rights::NONE };
    let want = pick(observe, Rights::OBSERVE) | pick(input, Rights::INPUT);
    let rights = pick(observe, Rights::RECEIVE) | pick(input, Rights::SEND);
    if !cx.store(rhizo_cap::ROOM) {
        return Err(-ENOMEM);
    }
    let mut created = false;
    let made = (|| {
        let hs = cx.net.handles().ok_or(-EACCES)?;
        let t = target(hs.grants, cell, g, label, want)?;
        let ch = hs
            .net
            .channels_mut()
            .create(hs.tree, Actor::Cell(cell), &|_| rhizo_cap::Limits::ZERO)
            .map_err(channel_err)?;
        // From here the channel's end gives its room back.
        created = true;
        let [e0, e1] = rhizo_cap::Channels::endpoints(ch);
        let ask = Ask {
            rights,
            holder: cell,
            ..Ask::default()
        };
        let object = Object::Endpoint(e0);
        let mine = match hs
            .grants
            .grant(hs.tree, hs.net.channels(), Actor::Cell(cell), object, ask)
        {
            Ok(x) => x,
            Err(e) => {
                hs.net.drop_channel(ch);
                return Err(grant_err(e));
            }
        };
        let s = hs.net.endpoint(e0, mine, rights);
        let held = s.and_then(|_| hs.net.hold(e1, t, g, observe, input));
        if let Err(e) = held {
            if let Ok(s) = s {
                hs.net.grant_gone(mine);
                hs.net.release(s);
            }
            let _ = hs.grants.revoke(hs.tree, Actor::Host, mine, &mut |_, _| {});
            hs.net.drop_channel(ch);
            return Err(net_err(e));
        }
        s.map_err(net_err)
    })();
    match made {
        Err(_) if !created => cx.frames.credit(rhizo_cap::ROOM),
        Ok(_) if observe => cx.net.observing(),
        _ => {}
    }
    made
}

/// `open` of the `/run/rhizo` entry `kept`, named `label`, by cell `cell`
/// for `access` (`O_RDONLY` 0, `O_WRONLY` 1, `O_RDWR` 2), non-blocking and
/// close-on-exec as `how` says: a descriptor of the endpoint it holds, of
/// the cell's inbox, or of a channel observing or giving input to the
/// cell a cell handle names ([`open_handle`]); `ENXIO` for any other
/// entry.
pub(crate) fn open<F, M, T, A: Frames, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    cell: u32,
    kept: Kept,
    label: &[u8],
    (access, nonblock, cloexec): (u64, bool, bool),
) -> i64 {
    let e = match kept {
        Kept::Socket(e) if is_packet(cx.net.net(), e) => {
            cx.net.net().retain(e);
            e
        }
        Kept::Inbox => match cx.net.net().inbox(cell) {
            Ok(e) => {
                cx.net.net().retain(e);
                e
            }
            Err(e) => return net_err(e),
        },
        Kept::Grant(g) => match open_handle(cx, cell, g, label, access) {
            Ok(e) => e,
            Err(x) => return x,
        },
        _ => return -ENXIO,
    };
    // Its descriptor counts as a socket's against the cell's memory
    // (design D4 of `add-cell-hierarchy`).
    let before = cx.held();
    if !cx.store(rhizo_net::SOCKET_BYTES) {
        cx.net.net().release(e);
        return -ENOMEM;
    }
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

/// The `/run/rhizo/cell/` entries cell `cell` unlinked: each handle that
/// is still the cell's under its label is dropped (design D9 of
/// `add-cell-hierarchy`), what was derived from it staying; what was
/// opened through it stays open until its descriptors close.
pub(crate) fn drop_entries<F, M, T, A, S>(cx: &mut Context<'_, F, M, T, A, S>, cell: u32) {
    let dropped = cx.fs.take_dropped();
    let Some(hs) = cx.net.handles() else {
        return;
    };
    for (g, label) in dropped {
        if target(hs.grants, cell, g, &label, Rights::NONE).is_ok() {
            hs.net.forget_grant(g);
            let _ = hs.grants.unlink(cell, g);
        }
    }
}

/// For endpoint socket `h`, granted to its holder by another cell (a
/// channel that cell opened to it, or a handle it sent), the label of the
/// holder's handle on that cell, or `?` when it holds none (design D10 of
/// `add-cell-hierarchy`); `None` for an endpoint the host or its holder
/// granted.
pub(crate) fn opener_label<F, M, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    h: Handle,
) -> Option<Vec<u8>> {
    let hs = cx.net.handles()?;
    let (_, g, _) = hs.net.endpoint_of(h)?;
    let x = hs.grants.get(g)?;
    let (holder, by) = (x.holder(), x.grantor());
    if by == rhizo_cap::HOST || by == holder {
        return None;
    }
    let label = hs
        .grants
        .iter()
        .map(|(_, y)| y)
        .find(|y| {
            y.holder() == holder
                && y.object() == Object::Cell(by)
                && !y.label().as_bytes().is_empty()
        })
        .map_or(&b"?"[..], |y| y.label().as_bytes());
    Some(label.to_vec())
}
