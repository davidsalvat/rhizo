//! Host tests of channel endpoints as `SOCK_SEQPACKET` descriptors and of
//! handles moved as `SCM_RIGHTS` (spec `cell-hierarchy`; design D8 to D10
//! of `add-cell-hierarchy`), through the system-call entry point, several
//! cells sharing the kernel's sockets, tree and grants.

use super::tests::{DATA, World};
use super::*;
use rhizo_cap::{Actor, Ask, Egress, Label, Limits, Object, Rights, Spec};
use rhizo_fs::{Kept, Place};

// AArch64 call numbers.
const OPENAT: u64 = 56;
const CLOSE: u64 = 57;
const PIPE2: u64 = 59;
const READ: u64 = 63;
const WRITE: u64 = 64;
const UNLINKAT: u64 = 35;
const RENAMEAT: u64 = 38;
const FCNTL: u64 = 25;
const PPOLL: u64 = 73;
const EPOLL_CREATE1: u64 = 20;
const EPOLL_CTL: u64 = 21;
const EPOLL_PWAIT: u64 = 22;
const SOCKET: u64 = 198;
const CONNECT: u64 = 203;
const GETSOCKOPT: u64 = 209;
const SENDTO: u64 = 206;
const RECVFROM: u64 = 207;
const SENDMSG: u64 = 211;
const RECVMSG: u64 = 212;

const AT_FDCWD: u64 = -100i64 as u64;
const O_NONBLOCK: u64 = 0x800;
const SOCK_SEQPACKET: u64 = 5;
const MSG_TRUNC: u64 = 0x20;
const MSG_CTRUNC: u32 = 0x8;
const POLLIN: u16 = 1;
const POLLOUT: u16 = 4;
const POLLHUP: u16 = 0x10;
const EROFS: i64 = 30;
const EMSGSIZE: i64 = 90;
const ETOOMANYREFS: i64 = 109;

/// Scratch addresses in the writable page.
const PATH: u64 = DATA;
const MSG: u64 = DATA + 0x40;
const IOV: u64 = DATA + 0x80;
const CTRL: u64 = DATA + 0xa0;
const OUT: u64 = DATA + 0x100;
const BUF: u64 = DATA + 0x200;

const ALL: Rights = Rights::SEND.union(Rights::RECEIVE).union(Rights::CARRY);
const TALK: Rights = Rights::SEND.union(Rights::RECEIVE);

/// Cells as the kernel holds them: each with its own memory and files,
/// all sharing the sockets, the tree and the grants, which sit in the
/// world of the cell that made the last call.
struct Cells {
    w: Vec<World>,
    at: usize,
}

impl Cells {
    /// Root cells `5, 6, …`, one per entry of `egress`, each with its
    /// policy as its ceiling.
    fn roots(egress: &[Egress]) -> Self {
        let mut w: Vec<World> = (0..egress.len())
            .map(|i| {
                let mut w = World::new(Arch::Aarch64);
                w.cell.id = 5 + i as u32;
                w
            })
            .collect();
        for (i, e) in egress.iter().enumerate() {
            let spec = Spec {
                egress: e.clone(),
                ..Spec::default()
            };
            w[0].tree.add_root(5 + i as u32, spec, 0).unwrap();
        }
        Self { w, at: 0 }
    }

    /// Cell `i`, the shared state moved into its world.
    fn on(&mut self, i: usize) -> &mut World {
        if i != self.at {
            let (a, b) = (self.at.min(i), self.at.max(i));
            let (lo, hi) = self.w.split_at_mut(b);
            let (x, y) = (&mut lo[a], &mut hi[0]);
            core::mem::swap(&mut x.net, &mut y.net);
            core::mem::swap(&mut x.tree, &mut y.tree);
            core::mem::swap(&mut x.grants, &mut y.grants);
            self.at = i;
        }
        &mut self.w[i]
    }

    fn id(&self, i: usize) -> u32 {
        self.w[i].cell.id
    }

    /// The world holding the shared state.
    fn kernel(&mut self) -> &mut World {
        &mut self.w[self.at]
    }

    /// A channel the host makes: its endpoints.
    fn channel(&mut self) -> [u32; 2] {
        let k = self.kernel();
        let ch = k
            .net
            .channels_mut()
            .create(&k.tree, Actor::Host, &|_| Limits::ZERO)
            .unwrap();
        rhizo_cap::Channels::endpoints(ch)
    }

    /// The host grants cell `i` endpoint `end` with `rights`, as
    /// `/run/rhizo/chan/<label>`; returns the grant.
    fn grant(&mut self, i: usize, end: u32, rights: Rights, label: &str) -> u32 {
        let holder = self.id(i);
        let k = self.kernel();
        let ask = Ask {
            rights,
            holder,
            label: Label::new(label.as_bytes()).unwrap(),
            ..Ask::default()
        };
        let g = k
            .grants
            .grant(
                &k.tree,
                k.net.channels(),
                Actor::Host,
                Object::Endpoint(end),
                ask,
            )
            .unwrap();
        let h = k.net.endpoint(end, g, rights).unwrap();
        self.w[i]
            .fs
            .rhizo_put(Place::Chan(label.as_bytes()), Kept::Socket(h))
            .unwrap();
        g
    }

    /// `by` revokes grant `g`, as the kernel does: the grants it takes read
    /// as closed.
    fn revoke(&mut self, by: Actor, g: u32) -> Result<(), rhizo_cap::Error> {
        let k = self.kernel();
        let mut gone = Vec::new();
        k.grants
            .revoke(&k.tree, by, g, &mut |id, _| gone.push(id))?;
        for id in gone {
            k.net.grant_gone(id);
        }
        Ok(())
    }

    /// What the kernel does after a call: the grants let go are dropped.
    fn settle(&mut self) {
        let mut dropped = Vec::new();
        for w in &mut self.w {
            dropped.extend(w.fs.take_dropped());
        }
        let k = self.kernel();
        dropped.extend(k.net.take_dropped());
        for g in dropped {
            if let Some(holder) = k.grants.get(g).map(rhizo_cap::Grant::holder) {
                k.grants.unlink(holder, g).unwrap();
            }
        }
    }

    /// The grants cell `i` holds, with their grantors.
    fn held(&self, i: usize) -> Vec<(u32, u32)> {
        let id = self.id(i);
        self.w[self.at]
            .grants
            .iter()
            .filter(|(_, g)| g.holder() == id)
            .map(|(n, g)| (n, g.grantor()))
            .collect()
    }
}

impl World {
    fn path(&mut self, p: &str) -> u64 {
        let mut b = p.as_bytes().to_vec();
        b.push(0);
        self.poke(PATH, &b);
        PATH
    }

    /// Opens `/run/rhizo/chan/<label>`.
    fn open_chan(&mut self, label: &str, flags: u64) -> i64 {
        let p = self.path(&format!("/run/rhizo/chan/{label}"));
        self.call(OPENAT, &[AT_FDCWD, p, flags]).0
    }

    /// Connects a new `SOCK_SEQPACKET` socket to `/run/rhizo/chan/<label>`.
    fn connect_chan(&mut self, label: &str) -> i64 {
        let s = self.call(SOCKET, &[1, SOCK_SEQPACKET, 0]).0;
        assert!(s >= 0);
        let mut addr = vec![1, 0];
        addr.extend_from_slice(format!("/run/rhizo/chan/{label}").as_bytes());
        addr.push(0);
        self.poke(OUT, &addr);
        match self.call(CONNECT, &[s as u64, OUT, addr.len() as u64]).0 {
            0 => s,
            e => e,
        }
    }

    fn write_msg(&mut self, fd: i64, data: &[u8]) -> i64 {
        self.poke(BUF, data);
        self.call(WRITE, &[fd as u64, BUF, data.len() as u64]).0
    }

    fn read_msg(&mut self, fd: i64, len: u64) -> Result<Vec<u8>, i64> {
        match self.call(READ, &[fd as u64, BUF, len]) {
            (n, Outcome::Resume) if n >= 0 => Ok(self.peek(BUF, n as usize)),
            (n, Outcome::Resume) => Err(n),
            (_, other) => panic!("read did not complete: {other:?}"),
        }
    }

    /// `sendmsg` of `data` on `fd` with the descriptors `fds` as
    /// `SCM_RIGHTS`.
    fn send_fds(&mut self, fd: i64, data: &[u8], fds: &[i64]) -> i64 {
        self.poke(BUF, data);
        let mut iov = BUF.to_le_bytes().to_vec();
        iov.extend_from_slice(&(data.len() as u64).to_le_bytes());
        self.poke(IOV, &iov);
        let mut cmsg = ((16 + 4 * fds.len()) as u64).to_le_bytes().to_vec();
        cmsg.extend_from_slice(&1u32.to_le_bytes());
        cmsg.extend_from_slice(&1u32.to_le_bytes());
        for &f in fds {
            cmsg.extend_from_slice(&(f as i32).to_le_bytes());
        }
        self.poke(CTRL, &cmsg);
        self.msghdr(cmsg.len() as u64);
        self.call(SENDMSG, &[fd as u64, MSG, 0]).0
    }

    /// A `msghdr` at `MSG`: one buffer at `IOV`, `control` bytes at `CTRL`.
    fn msghdr(&mut self, control: u64) {
        let mut h = vec![0u8; 56];
        h[16..24].copy_from_slice(&IOV.to_le_bytes());
        h[24..32].copy_from_slice(&1u64.to_le_bytes());
        h[32..40].copy_from_slice(&CTRL.to_le_bytes());
        h[40..48].copy_from_slice(&control.to_le_bytes());
        self.poke(MSG, &h);
    }

    /// `recvmsg` on `fd` into `len` bytes at `BUF` with `control` bytes of
    /// room: the result, the bytes, the descriptors received and the
    /// message flags.
    fn recv_fds(&mut self, fd: i64, len: u64, control: u64) -> (i64, Vec<u8>, Vec<i64>, u32) {
        let mut iov = BUF.to_le_bytes().to_vec();
        iov.extend_from_slice(&len.to_le_bytes());
        self.poke(IOV, &iov);
        self.msghdr(control);
        let n = self.call(RECVMSG, &[fd as u64, MSG, 0]).0;
        let h = self.peek(MSG, 56);
        let used = u64::from_le_bytes(h[40..48].try_into().unwrap());
        let flags = u32::from_le_bytes(h[48..52].try_into().unwrap());
        let mut fds = Vec::new();
        if n >= 0 && used >= 16 {
            let c = self.peek(CTRL, used as usize);
            let clen = u64::from_le_bytes(c[..8].try_into().unwrap());
            assert_eq!(
                &c[8..16],
                &[1, 0, 0, 0, 1, 0, 0, 0],
                "SOL_SOCKET, SCM_RIGHTS"
            );
            for i in 0..(clen as usize - 16) / 4 {
                let at = 16 + 4 * i;
                fds.push(i32::from_le_bytes(c[at..at + 4].try_into().unwrap()).into());
            }
        }
        let bytes = if n > 0 {
            self.peek(BUF, (n as u64).min(len) as usize)
        } else {
            Vec::new()
        };
        (n, bytes, fds, flags)
    }

    /// `ppoll` of `fd` for `IN` and `OUT`, returning its events at once.
    fn events(&mut self, fd: i64) -> u16 {
        let mut p = (fd as i32).to_le_bytes().to_vec();
        p.extend_from_slice(&(POLLIN | POLLOUT).to_le_bytes());
        p.extend_from_slice(&[0, 0]);
        self.poke(OUT, &p);
        self.poke(OUT + 16, &[0; 16]); // a zero timeout
        self.call(PPOLL, &[OUT, 1, OUT + 16, 0, 0]);
        u16::from_le_bytes(self.peek(OUT + 6, 2).try_into().unwrap())
    }
}

/// Spec `cell-hierarchy`, "Channels as descriptors": `open` and `connect`
/// of `/run/rhizo/chan/<label>` give descriptors of the endpoint, each
/// write is one message, each read takes one cut to the buffer, and
/// closing one end gives the other end of file and `EPIPE`.
#[test]
fn channels_carry_whole_messages() {
    let mut c = Cells::roots(&[Egress::None, Egress::None]);
    let [a, b] = c.channel();
    c.grant(0, a, TALK, "peer");
    c.grant(1, b, TALK, "peer");
    let x = c.on(0).connect_chan("peer");
    assert!(x >= 0, "connect gives a descriptor: {x}");
    assert_eq!(c.on(0).connect_chan("none"), -ENOENT);
    assert_eq!(c.on(0).write_msg(x, b"hello"), 5);
    assert_eq!(c.on(0).write_msg(x, b"cut short"), 9);
    assert_eq!(c.on(0).write_msg(x, b""), 0, "an empty message");
    assert_eq!(c.on(0).write_msg(x, b"last"), 4);
    let w = c.on(1);
    let y = w.open_chan("peer", 2);
    assert!(y >= 0);
    let y2 = w.open_chan("peer", 0);
    assert_eq!(w.read_msg(y, 100), Ok(b"hello".to_vec()));
    assert_eq!(
        w.read_msg(y2, 3),
        Ok(b"cut".to_vec()),
        "one queue, cut to the buffer"
    );
    let (n, bytes, fds, flags) = w.recv_fds(y, 100, 0);
    assert_eq!(
        (n, bytes, fds, flags),
        (0, vec![], vec![], 0),
        "the empty one"
    );
    assert_eq!(w.services.received, 3, "each message counts");
    w.poke(OUT + 32, &4u32.to_le_bytes());
    assert_eq!(w.call(GETSOCKOPT, &[y as u64, 1, 3, OUT, OUT + 32]).0, 0);
    assert_eq!(
        w.peek(OUT, 4),
        5u32.to_le_bytes(),
        "SO_TYPE: SOCK_SEQPACKET"
    );
    // A large message is refused whole.
    let big = rhizo_cap::limits::MESSAGE_MAX as u64 + 1;
    assert_eq!(w.call(WRITE, &[y as u64, BUF, big]).0, -EMSGSIZE);
    // The writer closes its descriptor and its entry: end of file once
    // the queue is read, and `EPIPE`.
    let w = c.on(0);
    assert_eq!(w.call(CLOSE, &[x as u64]).0, 0);
    let p = w.path("/run/rhizo/chan/peer");
    assert_eq!(w.call(UNLINKAT, &[AT_FDCWD, p, 0]).0, 0);
    c.settle();
    assert_eq!(c.held(0), [], "the handle was dropped");
    let w = c.on(1);
    let (n, bytes, _, flags) = w.recv_fds(y, 2, 0);
    assert_eq!((n, bytes, flags), (2, b"la".to_vec(), MSG_TRUNC as u32));
    assert_eq!(w.read_msg(y, 10), Ok(vec![]), "end of file");
    assert_eq!(w.write_msg(y, b"x"), -EPIPE);
    assert_eq!(w.events(y), POLLIN | POLLOUT | POLLHUP);
}

/// `MSG_TRUNC` asked returns a message's full length; a number written as
/// bytes names nothing in another cell (spec "A number is not a handle").
#[test]
fn channels_report_lengths_and_numbers_confer_nothing() {
    let mut c = Cells::roots(&[Egress::None, Egress::None]);
    let [a, b] = c.channel();
    c.grant(0, a, TALK, "peer");
    c.grant(1, b, TALK, "peer");
    let [s0, _] = c.channel();
    c.grant(0, s0, TALK, "secret");
    let x = c.on(0).open_chan("peer", 2);
    let secret = c.on(0).open_chan("secret", 2);
    let mut msg = (secret as u32).to_le_bytes().to_vec();
    msg.extend_from_slice(b"pad!");
    let w = c.on(0);
    w.poke(BUF, &msg);
    assert_eq!(w.call(SENDTO, &[x as u64, BUF, 8, 0, 0, 0]).0, 8);
    let w = c.on(1);
    let y = w.open_chan("peer", 2);
    assert_eq!(
        w.call(RECVFROM, &[y as u64, BUF, 4, MSG_TRUNC, 0, 0]).0,
        8,
        "the full length"
    );
    assert_eq!(w.peek(BUF + 4, 4), [0; 4], "cut to the buffer");
    // The receiver uses the number it was sent: it names nothing of the
    // sender's, here no descriptor at all.
    let number = i64::from(u32::from_le_bytes(w.peek(BUF, 4).try_into().unwrap()));
    assert_eq!(
        (number, y),
        (secret, x),
        "the same numbers, each cell's own"
    );
    assert_eq!(w.write_msg(number, b"x"), -EBADF);
}

/// Spec "Bounded queue": the 65th message to a peer that does not read
/// fails with `EAGAIN` in non-blocking mode, and once the peer reads one
/// the next send succeeds; a blocking reader waits on its endpoint's
/// queue and is woken by a message, as `poll` and `epoll` report.
#[test]
fn channels_poll_block_and_bound_their_queues() {
    let mut c = Cells::roots(&[Egress::None, Egress::None]);
    let [a, b] = c.channel();
    c.grant(0, a, TALK, "peer");
    c.grant(1, b, TALK, "peer");
    let y = c.on(1).open_chan("peer", 2);
    let w = c.on(1);
    assert_eq!(w.events(y), POLLOUT);
    let ep = w.call(EPOLL_CREATE1, &[0]).0;
    w.poke(OUT, &[1, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0]); // EPOLLIN, data 7
    assert_eq!(w.call(EPOLL_CTL, &[ep as u64, 1, y as u64, OUT]).0, 0);
    assert_eq!(w.call(EPOLL_PWAIT, &[ep as u64, OUT + 32, 4, 0, 0]).0, 0);
    let (_, out) = w.call(READ, &[y as u64, BUF, 10]);
    assert_eq!(out, Outcome::Block, "a blocking read waits");
    let id = w.cell.id;
    w.net.end_call(|| Some(id)); // the kernel puts it on the wait queues
    let x = c.on(0).open_chan("peer", 2 | O_NONBLOCK);
    for i in 0..64u8 {
        assert_eq!(c.on(0).write_msg(x, &[i]), 1);
    }
    let w = c.on(0);
    let mut woken = Vec::new();
    w.net.drain_woken(|k| woken.push(k));
    assert!(woken.contains(&6), "the reader's cell is woken");
    assert_eq!(w.write_msg(x, b"one more"), -EAGAIN);
    assert_eq!(w.events(x), 0, "no room");
    let w = c.on(1);
    assert_eq!(w.events(y), POLLIN | POLLOUT);
    assert_eq!(w.call(EPOLL_PWAIT, &[ep as u64, OUT + 32, 4, 0, 0]).0, 1);
    assert_eq!(w.peek(OUT + 40, 8), 7u64.to_le_bytes(), "the entry's data");
    assert_eq!(w.read_msg(y, 10), Ok(vec![0]));
    assert_eq!(c.on(0).write_msg(x, b"one more"), 8);
    // Non-blocking set later, as `fcntl` does.
    let w = c.on(1);
    for _ in 0..64 {
        w.read_msg(y, 10).unwrap();
    }
    assert_eq!(w.call(FCNTL, &[y as u64, 4, O_NONBLOCK]).0, 0);
    assert_eq!(w.read_msg(y, 10), Err(-EAGAIN));
}

/// Spec "Transfer over channels": a handle moves as `SCM_RIGHTS` on an
/// endpoint with the carry right, and the receiver holds a grant derived
/// from the sender's, revocable by it; descriptors of files, pipes,
/// sockets and `epoll` instances are refused with `EPERM`.
#[test]
fn channels_move_handles_as_scm_rights() {
    let mut c = Cells::roots(&[Egress::None, Egress::None]);
    let [c0, c1] = c.channel();
    let [t0, t1] = c.channel();
    c.grant(0, c0, ALL, "carry");
    c.grant(1, c1, ALL, "carry");
    let tool = c.grant(0, t0, TALK, "tool");
    c.grant(0, t1, TALK, "back");
    let w = c.on(0);
    let carry = w.open_chan("carry", 2);
    let t = w.open_chan("tool", 2);
    // Only handles are sent.
    let file = {
        let p = w.path("/f");
        w.call(OPENAT, &[AT_FDCWD, p, 0x42]).0
    };
    assert_eq!(w.call(PIPE2, &[OUT, 0]).0, 0);
    let pipe = i64::from(i32::from_le_bytes(w.peek(OUT, 4).try_into().unwrap()));
    let tcp = w.call(SOCKET, &[2, 1, 0]).0;
    let ep = w.call(EPOLL_CREATE1, &[0]).0;
    for (fd, what) in [
        (file, "file"),
        (pipe, "pipe"),
        (tcp, "socket"),
        (ep, "epoll"),
        (0, "console"),
    ] {
        assert_eq!(w.send_fds(carry, b"x", &[fd]), -EPERM, "{what}");
    }
    assert_eq!(w.send_fds(carry, b"x", &[99]), -EBADF);
    assert_eq!(
        w.send_fds(carry, b"x", &[t; 17]),
        -ETOOMANYREFS,
        "at most 16 handles"
    );
    assert_eq!(w.send_fds(carry, b"take", &[t]), 4);
    c.settle();
    let w = c.on(1);
    let y = w.open_chan("carry", 2);
    let (n, bytes, fds, flags) = w.recv_fds(y, 64, 64);
    assert_eq!((n, bytes.as_slice(), flags), (4, &b"take"[..], 0));
    assert_eq!(fds.len(), 1);
    let got = fds[0];
    assert_eq!(w.write_msg(got, b"via B"), 5);
    let mine = c.held(1);
    assert_eq!(mine.len(), 2, "carry and the handle it took");
    let (g, grantor) = mine
        .iter()
        .copied()
        .find(|&(g, _)| c.kernel().grants.get(g).unwrap().parent() == Some(tool))
        .unwrap();
    assert_eq!(grantor, c.id(0), "the sender may revoke it");
    let w = c.on(0);
    let back = w.open_chan("back", 2);
    assert_eq!(w.read_msg(back, 64), Ok(b"via B".to_vec()));
    // A sender without the carry right.
    let [d0, d1] = c.channel();
    c.grant(0, d0, TALK, "plain");
    c.grant(1, d1, TALK, "plain");
    let w = c.on(0);
    let plain = w.open_chan("plain", 2);
    assert_eq!(w.send_fds(plain, b"x", &[t]), -EACCES);
    assert_eq!(
        w.send_fds(plain, b"x", &[]),
        1,
        "no handle, no carry needed"
    );
    // Handles that find no room are let go (`MSG_CTRUNC`), as is a
    // handle read with `read`.
    let before = c.held(1).len();
    let w = c.on(0);
    assert_eq!(w.send_fds(carry, b"two", &[t, t]), 3);
    assert_eq!(w.send_fds(carry, b"lost", &[t]), 4);
    let w = c.on(1);
    let (n, _, fds, flags) = w.recv_fds(y, 64, 20);
    assert_eq!((n, fds.len(), flags), (3, 1, MSG_CTRUNC), "room for one");
    assert_eq!(w.read_msg(y, 64), Ok(b"lost".to_vec()));
    c.settle();
    assert_eq!(c.held(1).len(), before + 1, "only the handle with room");
    // The grant revoked by its grantor: the receiver's descriptor reads
    // as closed.
    c.revoke(Actor::Cell(c.id(0)), g).unwrap();
    let w = c.on(1);
    assert_eq!(w.read_msg(got, 8), Ok(vec![]));
    assert_eq!(w.write_msg(got, b"x"), -EPIPE);
}

/// Spec "Revoked while passed on" and "Passed on, then closed": A grants
/// B a channel, B passes it to C; B closing its own leaves C's working
/// until A revokes the grant, then both read end of file.
#[test]
fn channels_revoked_while_passed_on() {
    let mut c = Cells::roots(&[Egress::None, Egress::None, Egress::None]);
    let [t0, t1] = c.channel();
    let [c0, c1] = c.channel();
    let b_tool = c.grant(1, t0, TALK, "tool");
    c.grant(0, t1, TALK, "tool");
    c.grant(1, c0, ALL, "carry");
    c.grant(2, c1, ALL, "carry");
    let w = c.on(1);
    let carry = w.open_chan("carry", 2);
    let tool = w.open_chan("tool", 2);
    assert_eq!(w.send_fds(carry, b"", &[tool]), 0);
    let w = c.on(2);
    let y = w.open_chan("carry", 2);
    let (_, _, fds, _) = w.recv_fds(y, 8, 64);
    let ctool = fds[0];
    // B lets its own go: C keeps using the channel.
    let w = c.on(1);
    assert_eq!(w.call(CLOSE, &[tool as u64]).0, 0);
    let p = w.path("/run/rhizo/chan/tool");
    assert_eq!(w.call(UNLINKAT, &[AT_FDCWD, p, 0]).0, 0);
    c.settle();
    assert_eq!(c.on(2).write_msg(ctool, b"from C"), 6);
    let a = c.on(0).open_chan("tool", 2);
    assert_eq!(c.on(0).read_msg(a, 64), Ok(b"from C".to_vec()));
    assert!(
        c.kernel().grants.get(b_tool).is_none(),
        "B's grant was dropped"
    );
    // C's grant, kept when B dropped its own, is revoked by whoever
    // could revoke B's (here the host), not by C.
    let cg = c
        .held(2)
        .into_iter()
        .find(|&(_, by)| by == c.id(1))
        .unwrap()
        .0;
    assert_eq!(
        c.revoke(Actor::Cell(c.id(2)), cg),
        Err(rhizo_cap::Error::Denied),
        "C cannot revoke what B sent it"
    );
    c.revoke(Actor::Host, cg).unwrap();
    let w = c.on(2);
    assert_eq!(w.read_msg(ctool, 8), Ok(vec![]));
    assert_eq!(w.write_msg(ctool, b"x"), -EPIPE);
    // Revoked while passed on: both copies end together.
    let [u0, u1] = c.channel();
    let b_u = c.grant(1, u0, TALK, "u");
    c.grant(0, u1, TALK, "u");
    let w = c.on(1);
    let bu = w.open_chan("u", 2);
    assert_eq!(w.send_fds(carry, b"", &[bu]), 0);
    let w = c.on(2);
    let (_, _, fds, _) = w.recv_fds(y, 8, 64);
    let cu = fds[0];
    c.revoke(Actor::Host, b_u).unwrap();
    assert_eq!(c.on(1).read_msg(bu, 8), Ok(vec![]));
    assert_eq!(c.on(1).write_msg(bu, b"x"), -EPIPE);
    assert_eq!(c.on(2).read_msg(cu, 8), Ok(vec![]));
    assert_eq!(c.on(2).write_msg(cu, b"x"), -EPIPE);
}

/// Spec "No proxy to the internet", over a transfer: a handle whose
/// peer has egress `full` is not delivered to a cell whose ceiling is
/// `none` (`EACCES`), and nothing is sent.
#[test]
fn channels_refuse_a_proxy_past_the_ceiling() {
    let mut c = Cells::roots(&[]);
    let mut w = World::new(Arch::Aarch64);
    w.cell.id = 1;
    let limits = Limits {
        memory: 1 << 30,
        cpu_ns: u64::MAX,
        sockets: 1024,
        cells: 64,
    };
    let full = Spec {
        egress: Egress::Full,
        limits,
        ..Spec::default()
    };
    w.tree.add_root(1, full.clone(), 0).unwrap(); // P
    let small = Limits {
        memory: 1 << 20,
        sockets: 4,
        cells: 1,
        ..limits
    };
    let none = |l| Spec {
        limits: l,
        ..Spec::default()
    };
    w.tree
        .add_child(2, 1, none(small), Actor::Host, &|_| Limits::ZERO)
        .unwrap(); // B
    let s = Spec {
        limits: small,
        ..full
    };
    w.tree
        .add_child(3, 1, s, Actor::Host, &|_| Limits::ZERO)
        .unwrap(); // S
    c.w.push(w);
    for id in [2, 3] {
        let mut w = World::new(Arch::Aarch64);
        w.cell.id = id;
        c.w.push(w);
    }
    let [c0, c1] = c.channel();
    let [t0, t1] = c.channel();
    c.grant(0, c0, ALL, "carry");
    c.grant(1, c1, ALL, "carry");
    c.grant(0, t0, TALK, "tool");
    c.grant(2, t1, TALK, "tool");
    let w = c.on(0);
    let carry = w.open_chan("carry", 2);
    let tool = w.open_chan("tool", 2);
    let before = c.kernel().grants.clone();
    assert_eq!(c.on(0).send_fds(carry, b"x", &[tool]), -EACCES);
    assert_eq!(c.kernel().grants, before, "no grant made");
    let w = c.on(1);
    let y = w.open_chan("carry", 2 | O_NONBLOCK);
    assert_eq!(w.read_msg(y, 8), Err(-EAGAIN), "nothing sent");
}

/// A handle sent to one holder of an endpoint and taken by another holder
/// of it becomes the taker's.
#[test]
fn channels_hand_a_handle_to_the_holder_that_takes_it() {
    let mut c = Cells::roots(&[Egress::None, Egress::None, Egress::None]);
    let [c0, c1] = c.channel();
    let [t0, _] = c.channel();
    c.grant(0, c0, ALL, "carry");
    c.grant(1, c1, ALL, "carry");
    c.grant(2, c1, ALL, "carry");
    c.grant(0, t0, TALK, "tool");
    let w = c.on(0);
    let carry = w.open_chan("carry", 2);
    let tool = w.open_chan("tool", 2);
    assert_eq!(w.send_fds(carry, b"", &[tool]), 0);
    // The grant was made for C, the newest holder of the peer endpoint.
    let held = |c: &Cells, i| c.held(i).len();
    let (b, cc) = (held(&c, 1), held(&c, 2));
    let w = c.on(1);
    let y = w.open_chan("carry", 2);
    let (_, _, fds, _) = w.recv_fds(y, 8, 64);
    assert_eq!(fds.len(), 1);
    assert_eq!(
        (held(&c, 1), held(&c, 2)),
        (b + 1, cc - 1),
        "B took it: it is B's"
    );
    assert_eq!(c.on(1).write_msg(fds[0], b"ok"), 2);
}

/// Spec "Rename refused", through the calls: `/run/rhizo` cannot be
/// changed but by unlinking an entry, and opening one for writing fails.
#[test]
fn channels_entries_are_read_only() {
    let mut c = Cells::roots(&[Egress::None]);
    let [a, _] = c.channel();
    c.grant(0, a, TALK, "tools");
    let w = c.on(0);
    let mut paths = b"/run/rhizo/chan/tools\0/run/rhizo/chan/x\0".to_vec();
    paths.resize(64, 0);
    w.poke(PATH, &paths);
    let (old, new) = (PATH, PATH + 22);
    assert_eq!(w.call(RENAMEAT, &[AT_FDCWD, old, AT_FDCWD, new]).0, -EROFS);
    assert_eq!(w.call(OPENAT, &[AT_FDCWD, new, 0x41]).0, -EROFS, "create");
    assert!(w.open_chan("tools", 0) >= 0, "the entry is unchanged");
    let p = w.path("/run/rhizo/generation");
    let fd = w.call(OPENAT, &[AT_FDCWD, p, 0]).0;
    assert_eq!(
        w.read_msg(fd, 16).unwrap().len(),
        8,
        "the generation, as before"
    );
}
