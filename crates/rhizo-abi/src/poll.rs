//! Pipes, event counters and waiting on descriptors (spec `linux-abi`:
//! pipes and event counters, non-blocking descriptors, waiting on
//! descriptors; design D3 and D5 of `add-sockets-and-poll`).

use super::*;
use crate::files::err;
use alloc::vec;
use alloc::vec::Vec;
use rhizo_fs::{EpollOp, Error};

const O_NONBLOCK: u64 = 0x800;
const O_CLOEXEC: u64 = 0x8_0000;

pub(crate) const POLLIN: u32 = 0x1;
pub(crate) const POLLOUT: u32 = 0x4;
pub(crate) const POLLERR: u32 = 0x8;
pub(crate) const POLLHUP: u32 = 0x10;
const POLLNVAL: u32 = 0x20;
const POLLRDNORM: u32 = 0x40;
const POLLWRNORM: u32 = 0x100;
const EPOLLONESHOT: u32 = 1 << 30;
/// Flags of an `epoll` entry that are not events.
const EPOLL_FLAGS: u32 = 0xf000_0000;

/// Bytes copied on the kernel stack before falling back to the heap.
const STACK_COPY: usize = 512;

/// Runs `f` with the `len` bytes of cell memory at `addr` (`-EFAULT` if
/// they cannot be read).
pub(crate) fn with_user_bytes<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    addr: u64,
    len: usize,
    f: impl FnOnce(&mut Context<'_, F, M, T, A, S>, &[u8]) -> i64,
) -> i64 {
    if len <= STACK_COPY {
        let mut buf = [0u8; STACK_COPY];
        if cx.read(addr, &mut buf[..len]).is_err() {
            return -EFAULT;
        }
        f(cx, &buf[..len])
    } else {
        let mut buf = vec![0u8; len];
        if cx.read(addr, &mut buf).is_err() {
            return -EFAULT;
        }
        f(cx, &buf)
    }
}

/// What a call that would wait returns: `-EAGAIN` on a non-blocking
/// descriptor or once `timeout` passed, else [`BLOCK`].
pub(crate) fn would_wait<F, M, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    nonblocking: bool,
    timeout: Option<u64>,
) -> i64 {
    if nonblocking {
        return -EAGAIN;
    }
    block(cx, timeout).unwrap_or(-EAGAIN)
}

pub(crate) fn pipe2<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    addr: u64,
    flags: u64,
) -> i64 {
    if flags & !(O_NONBLOCK | O_CLOEXEC) != 0 {
        return -EINVAL;
    }
    let (r, w) = match cx.fs.pipe(flags & O_NONBLOCK != 0, flags & O_CLOEXEC != 0) {
        Ok(ends) => ends,
        Err(e) => return err(e),
    };
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&r.to_le_bytes());
    out[4..].copy_from_slice(&w.to_le_bytes());
    if cx.write(addr, &out).is_err() {
        let _ = cx.fs.close(r);
        let _ = cx.fs.close(w);
        return -EFAULT;
    }
    0
}

pub(crate) fn eventfd2<F, M, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    count: u64,
    flags: u64,
) -> i64 {
    const EFD_SEMAPHORE: u64 = 1;
    if flags & !(EFD_SEMAPHORE | O_NONBLOCK | O_CLOEXEC) != 0 {
        return -EINVAL;
    }
    let semaphore = flags & EFD_SEMAPHORE != 0;
    match cx.fs.eventfd(
        u64::from(count as u32),
        semaphore,
        flags & O_NONBLOCK != 0,
        flags & O_CLOEXEC != 0,
    ) {
        Ok(fd) => fd as i64,
        Err(e) => err(e),
    }
}

pub(crate) fn read_pipe<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u32,
    addr: u64,
    len: u64,
) -> i64 {
    if len == 0 {
        return 0;
    }
    let (pt, mem, pager) = (&cx.pt, &mut *cx.mem, &mut *cx.pager);
    let (mut done, mut faulted) = (0u64, false);
    let got = cx
        .fs
        .read_pipe(fd, len.min(usize::MAX as u64) as usize, &mut |piece| {
            faulted = memory::write(pt, mem, pager, addr.wrapping_add(done), piece).is_err();
            done += piece.len() as u64;
            !faulted
        });
    match got {
        Ok(0) if faulted => -EFAULT,
        Ok(n) => n as i64,
        Err(Error::Again) => {
            let nonblocking = cx.fs.nonblocking(fd).unwrap_or(true);
            would_wait(cx, nonblocking, None)
        }
        Err(e) => err(e),
    }
}

pub(crate) fn write_pipe<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u32,
    addr: u64,
    len: u64,
) -> i64 {
    if len == 0 {
        return 0;
    }
    let n = len.min(rhizo_fs::PIPE_CAP as u64) as usize;
    with_user_bytes(cx, addr, n, |cx, bytes| match cx.fs.write_pipe(fd, bytes) {
        Ok(n) => n as i64,
        Err(Error::Again) => {
            let nonblocking = cx.fs.nonblocking(fd).unwrap_or(true);
            would_wait(cx, nonblocking, None)
        }
        Err(e) => err(e),
    })
}

pub(crate) fn read_event<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u32,
    addr: u64,
    len: u64,
) -> i64 {
    if len < 8 {
        return -EINVAL;
    }
    match cx.fs.read_event(fd) {
        Ok(value) => match put(cx, addr, &value.to_le_bytes()) {
            0 => 8,
            e => e,
        },
        Err(Error::Again) => {
            let nonblocking = cx.fs.nonblocking(fd).unwrap_or(true);
            would_wait(cx, nonblocking, None)
        }
        Err(e) => err(e),
    }
}

pub(crate) fn write_event<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u32,
    addr: u64,
    len: u64,
) -> i64 {
    if len < 8 {
        return -EINVAL;
    }
    let mut value = [0u8; 8];
    if cx.read(addr, &mut value).is_err() {
        return -EFAULT;
    }
    match cx.fs.write_event(fd, u64::from_le_bytes(value)) {
        Ok(()) => 8,
        Err(Error::Again) => {
            let nonblocking = cx.fs.nonblocking(fd).unwrap_or(true);
            would_wait(cx, nonblocking, None)
        }
        Err(e) => err(e),
    }
}

/// The events of `fd`, as `poll` reports them (`POLLRDNORM` and
/// `POLLWRNORM` with `POLLIN` and `POLLOUT`).
fn events_of<F, M, T, A, S>(cx: &Context<'_, F, M, T, A, S>, fd: u32) -> Result<u32, Error> {
    let net = &*cx.net;
    let ev = cx.fs.poll(fd, &|h| net.poll(h))?;
    Ok(ev
        | if ev & POLLIN != 0 { POLLRDNORM } else { 0 }
        | if ev & POLLOUT != 0 { POLLWRNORM } else { 0 })
}

/// A `timespec` timeout at `addr` (null: none), in nanoseconds.
pub(crate) fn timespec_timeout<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    addr: u64,
) -> Result<Option<u64>, i64> {
    timeout_at(cx, addr, 1_000_000_000)
}

/// A `timeval` timeout at `addr` (null: none), in nanoseconds.
pub(crate) fn timeval_timeout<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    addr: u64,
) -> Result<Option<u64>, i64> {
    timeout_at(cx, addr, 1_000_000)
}

fn timeout_at<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    addr: u64,
    per_second: i64,
) -> Result<Option<u64>, i64> {
    if addr == 0 {
        return Ok(None);
    }
    let mut t = [0u8; 16];
    cx.read(addr, &mut t).map_err(|_| -EFAULT)?;
    let sec = i64::from_le_bytes(t[..8].try_into().unwrap_or_default());
    let frac = i64::from_le_bytes(t[8..].try_into().unwrap_or_default());
    if sec < 0 || !(0..per_second).contains(&frac) {
        return Err(-EINVAL);
    }
    Ok(Some(
        (sec as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add(frac as u64 * (1_000_000_000 / per_second as u64)),
    ))
}

/// `poll`/`ppoll` (the signal mask is ignored: no signals are delivered).
pub(crate) fn poll<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    addr: u64,
    nfds: u64,
    timeout: Option<u64>,
) -> i64 {
    if nfds > rhizo_fs::FD_MAX as u64 {
        return -EINVAL;
    }
    let mut fds = vec![0u8; nfds as usize * 8];
    if cx.read(addr, &mut fds).is_err() {
        return -EFAULT;
    }
    let mut ready = 0;
    for entry in fds.chunks_exact_mut(8) {
        let fd = i32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
        let wanted = u32::from(u16::from_le_bytes([entry[4], entry[5]]));
        let revents = if fd < 0 {
            0
        } else {
            match events_of(cx, fd as u32) {
                Ok(ev) => ev & (wanted | POLLERR | POLLHUP),
                Err(_) => POLLNVAL,
            }
        };
        entry[6..8].copy_from_slice(&(revents as u16).to_le_bytes());
        ready += i64::from(revents != 0);
    }
    if ready == 0
        && timeout != Some(0)
        && let Some(b) = block(cx, timeout)
    {
        return b;
    }
    match cx.write(addr, &fds) {
        Ok(()) => ready,
        Err(_) => -EFAULT,
    }
}

/// `select`/`pselect6` with descriptors: `a` = nfds, read, write and
/// exception sets.
pub(crate) fn select<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    a: [u64; 6],
    timeout: Option<u64>,
) -> i64 {
    let nfds = a[0] as i32;
    if nfds < 0 || nfds as usize > rhizo_fs::FD_MAX {
        return -EINVAL;
    }
    let bytes = (nfds as usize).div_ceil(64) * 8;
    let mut sets = [vec![0u8; bytes], vec![0u8; bytes], vec![0u8; bytes]];
    for (set, &addr) in sets.iter_mut().zip(&a[1..4]) {
        if addr != 0 && cx.read(addr, set).is_err() {
            return -EFAULT;
        }
    }
    let mut out = [vec![0u8; bytes], vec![0u8; bytes], vec![0u8; bytes]];
    let mut ready = 0;
    for fd in 0..nfds as usize {
        let (byte, bit) = (fd / 8, 1u8 << (fd % 8));
        if sets.iter().all(|s| s[byte] & bit == 0) {
            continue;
        }
        let ev = match events_of(cx, fd as u32) {
            Ok(ev) => ev,
            Err(e) => return err(e),
        };
        let hits = [
            ev & (POLLIN | POLLHUP | POLLERR) != 0,
            ev & (POLLOUT | POLLERR) != 0,
            false, // no urgent data
        ];
        for i in 0..3 {
            if sets[i][byte] & bit != 0 && hits[i] {
                out[i][byte] |= bit;
                ready += 1;
            }
        }
    }
    if ready == 0
        && timeout != Some(0)
        && let Some(b) = block(cx, timeout)
    {
        return b;
    }
    for (set, &addr) in out.iter().zip(&a[1..4]) {
        if addr != 0 && cx.write(addr, set).is_err() {
            return -EFAULT;
        }
    }
    ready
}

pub(crate) fn epoll_create<F, M, T, A, S>(cx: &mut Context<'_, F, M, T, A, S>, flags: u64) -> i64 {
    if flags & !O_CLOEXEC != 0 {
        return -EINVAL;
    }
    match cx.fs.epoll_create(flags != 0) {
        Ok(fd) => fd as i64,
        Err(e) => err(e),
    }
}

pub(crate) fn epoll_ctl<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    arch: Arch,
    epfd: u64,
    op: u64,
    fd: u64,
    event: u64,
) -> i64 {
    let op = match op {
        1 => EpollOp::Add,
        2 => EpollOp::Delete,
        3 => EpollOp::Modify,
        _ => return -EINVAL,
    };
    let (mut events, mut data) = (0, 0);
    if op != EpollOp::Delete {
        let (size, at) = arch.epoll_event();
        let mut e = [0u8; 16];
        if cx.read(event, &mut e[..size]).is_err() {
            return -EFAULT;
        }
        events = u32::from_le_bytes([e[0], e[1], e[2], e[3]]);
        data = u64::from_le_bytes(e[at..at + 8].try_into().unwrap_or_default());
    }
    match cx.fs.epoll_ctl(epfd as u32, op, fd as u32, events, data) {
        Ok(()) => 0,
        Err(e) => err(e),
    }
}

impl Arch {
    /// Size of `struct epoll_event` and offset of its data.
    const fn epoll_event(self) -> (usize, usize) {
        match self {
            Arch::X86_64 => (12, 4), // packed
            Arch::Aarch64 => (16, 8),
        }
    }
}

/// `epoll_wait`/`epoll_pwait` (the signal mask is ignored).
pub(crate) fn epoll_wait<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    arch: Arch,
    epfd: u64,
    addr: u64,
    max: u64,
    timeout_ms: u64,
) -> i64 {
    let max = max as i32;
    if max <= 0 {
        return -EINVAL;
    }
    let interests: Vec<rhizo_fs::Interest> = match cx.fs.epoll_interests(epfd as u32) {
        Ok(list) => list.to_vec(),
        Err(e) => return err(e),
    };
    let mut out = Vec::new();
    for e in interests {
        if out.len() == max as usize {
            break;
        }
        if e.events & !EPOLL_FLAGS == 0 {
            continue; // a one-shot entry that already fired
        }
        let Ok(ev) = events_of(cx, e.fd) else {
            continue;
        };
        let hit = ev & (e.events | POLLERR | POLLHUP) & !EPOLL_FLAGS;
        if hit != 0 {
            out.push((hit, e.data, e.fd, e.events & EPOLLONESHOT != 0));
        }
    }
    if out.is_empty() {
        let ms = timeout_ms as i32;
        let timeout = (ms >= 0).then(|| ms as u64 * 1_000_000);
        if timeout != Some(0)
            && let Some(b) = block(cx, timeout)
        {
            return b;
        }
        return 0;
    }
    let (size, at) = arch.epoll_event();
    let mut bytes = vec![0u8; out.len() * size];
    for (slot, &(events, data, _, _)) in bytes.chunks_exact_mut(size).zip(&out) {
        slot[..4].copy_from_slice(&events.to_le_bytes());
        slot[at..at + 8].copy_from_slice(&data.to_le_bytes());
    }
    if cx.write(addr, &bytes).is_err() {
        return -EFAULT;
    }
    for &(_, _, fd, oneshot) in &out {
        if oneshot {
            let _ = cx.fs.epoll_mask(epfd as u32, fd, EPOLL_FLAGS);
        }
    }
    out.len() as i64
}
