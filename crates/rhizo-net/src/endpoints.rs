//! Channel endpoints as sockets (design D9 and D10 of `add-cell-hierarchy`).
//!
//! The kernel's [`Channels`] live here, with the sockets: an endpoint a
//! cell holds is a connected `AF_UNIX` `SOCK_SEQPACKET` socket, one per
//! grant, which file descriptions share as they share any socket. So an
//! endpoint is polled, waited on and woken, released and copied through
//! the paths every socket takes: a message wakes only the wait queues of
//! the sockets holding its endpoint (design D1 of `add-socket-wait-queues`).
//!
//! The sockets on one endpoint form a list (through [`State::Endpoint`]);
//! the endpoint closes with the last of them, and the handles still queued
//! on it are released. A handle in flight is a socket too: a message
//! carries the sockets the kernel made for the grants it moves, each with
//! the reference the receiver's descriptor takes over. Grants themselves
//! are the kernel's business ([`rhizo_cap::Grants`]): a socket names its
//! grant, the grants whose last socket closed are handed back
//! ([`Net::take_dropped`]), and a grant revoked or gone makes its sockets
//! read as closed ([`Net::grant_gone`]).
//!
//! Cell handles (task 2.5) add three things here. Each cell's inbox is a
//! listening sequenced-packet socket where channels opened to it wait
//! ([`Net::inbox`], [`Net::deliver`]). An endpoint the kernel holds for a
//! cell, no socket on it, carries the cell's output to an observer
//! ([`Net::tell`]) or takes a sender's messages as its standard input
//! ([`Net::take_input`]), never making the cell wait. And a channel a cell
//! made gives its reserved room back when it ends ([`Net::take_refunds`]).

use super::*;
use rhizo_cap::limits::{INBOX_MAX, MESSAGE_HANDLES_MAX};
use rhizo_cap::{Error as CapError, Recv};

/// No socket.
pub(crate) const NO_SOCKET: Handle = Handle::MAX;

impl Net {
    /// The kernel's channels.
    pub fn channels(&self) -> &Channels {
        &self.channels
    }

    /// The kernel's channels, to make one (`Channels::create`).
    pub fn channels_mut(&mut self) -> &mut Channels {
        &mut self.channels
    }

    /// A new socket, with one reference, holding endpoint `end` for grant
    /// `grant` with its `rights`. `Invalid` if the endpoint is closed.
    pub fn endpoint(&mut self, end: u32, grant: u32, rights: Rights) -> Result<Handle> {
        if self.channels.is_open(end) != Some(true) {
            return Err(Error::Invalid);
        }
        let i = end as usize;
        if self.ends.len() <= i {
            self.ends.resize(i + 1, NO_SOCKET);
        }
        let mut s = Socket::new(
            Family::Unix,
            Type::SeqPacket,
            Options::default(),
            self.caller_net(),
        );
        s.state = State::Endpoint {
            end,
            grant,
            rights,
            next: self.ends[i],
        };
        let h = self.insert(s)?;
        self.ends[i] = h;
        Ok(h)
    }

    /// The endpoint `h` holds, its grant and the grant's rights; `None`
    /// for any other socket (an endpoint that reads as closed included).
    pub fn endpoint_of(&self, h: Handle) -> Option<(u32, u32, Rights)> {
        match self.get(h).ok()?.state {
            State::Endpoint {
                end, grant, rights, ..
            } => Some((end, grant, rights)),
            _ => None,
        }
    }

    /// The sockets holding endpoint `end`, the newest first.
    fn holders(&self, end: u32) -> impl Iterator<Item = Handle> + '_ {
        let mut at = self.ends.get(end as usize).copied().unwrap_or(NO_SOCKET);
        core::iter::from_fn(move || {
            let h = at;
            match self.get(h).map(|s| &s.state) {
                Ok(&State::Endpoint { next, .. }) => {
                    at = next;
                    Some(h)
                }
                _ => None,
            }
        })
    }

    /// The grants of the sockets holding endpoint `end`, the newest first.
    pub fn end_grants(&self, end: u32) -> impl Iterator<Item = u32> + '_ {
        self.holders(end)
            .filter_map(|h| self.endpoint_of(h).map(|(_, g, _)| g))
    }

    /// Wakes the waiters of every socket holding endpoint `end`.
    fn touch_end(&mut self, end: u32) {
        let mut at = self.ends.get(end as usize).copied().unwrap_or(NO_SOCKET);
        while let Ok(&State::Endpoint { next, .. }) = self.get(at).map(|s| &s.state) {
            self.touch(at);
            at = next;
        }
    }

    /// The live endpoint of `h` and its rights: `Ok(None)` if it reads as
    /// closed (revoked, or copied with its cell), `NotConn` if `h` is a
    /// sequenced-packet socket not connected to any.
    fn end_of(&self, h: Handle) -> Result<Option<(u32, Rights)>> {
        match self.get(h)?.state {
            State::Endpoint { end, rights, .. } => Ok(Some((end, rights))),
            State::Reset { .. } => Ok(None),
            _ => Err(Error::NotConn),
        }
    }

    /// Sends one message on endpoint socket `h`: `data` and the endpoint
    /// sockets `handles` (each reference passes to the message), waking the
    /// peer's waiters. Refused, changing nothing: without the send right
    /// (or the carry right, with handles) `Access`; a full peer queue
    /// `Again`; a closed peer `Pipe`; `MsgSize` past 64 KiB; `TooManyRefs`
    /// past 16 handles.
    pub fn send_message(&mut self, h: Handle, data: &[u8], handles: &[Handle]) -> Result<()> {
        self.look(h);
        let Some((end, rights)) = self.end_of(h)? else {
            return Err(Error::Pipe);
        };
        if !rights.contains(Rights::SEND)
            || (!handles.is_empty() && !rights.contains(Rights::CARRY))
        {
            return Err(Error::Access);
        }
        let sent = self.channels.send(end, data, handles);
        // Input for a cell, to an endpoint the kernel holds: never waits
        // for it (what does not fit is dropped), and the cell is woken to
        // take it.
        if !self.held.is_empty()
            && let Some(x) = self.held.iter().find(|x| x.end == end ^ 1 && x.input)
            && matches!(sent, Ok(()) | Err(CapError::Full))
        {
            let cell = x.cell;
            if sent.is_ok() && !self.inputs.contains(&cell) {
                self.inputs.push(cell);
            }
            self.changes += 1;
            self.woken.push(cell);
            return Ok(());
        }
        sent.map_err(|e| match e {
            CapError::Full => Error::Again,
            CapError::Pipe => Error::Pipe,
            CapError::TooBig => Error::MsgSize,
            CapError::TooManyRefs => Error::TooManyRefs,
            _ => Error::Invalid,
        })?;
        self.touch_end(end ^ 1);
        Ok(())
    }

    /// Receives the oldest message queued on endpoint socket `h`: at most
    /// `max` bytes handed to `take` in pieces (the rest of the message is
    /// dropped), its handles (sockets, each with a reference for the
    /// receiver) into `handles`, waking the peer's waiters for room.
    /// `Again` when nothing is queued; [`Recv::End`] once the peer closed
    /// and its messages were read, or when `h` reads as closed; `Access`
    /// without the receive right.
    pub fn recv_message(
        &mut self,
        h: Handle,
        max: usize,
        handles: &mut [u32; MESSAGE_HANDLES_MAX as usize],
        take: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Result<Recv> {
        self.look(h);
        let Some((end, rights)) = self.end_of(h)? else {
            return Ok(Recv::End);
        };
        if !rights.contains(Rights::RECEIVE) {
            return Err(Error::Access);
        }
        match self.channels.recv_with(end, max, handles, take) {
            Ok(Recv::Empty) => Err(Error::Again),
            Ok(r @ Recv::Message { .. }) => {
                self.touch_end(end ^ 1);
                self.received();
                Ok(r)
            }
            Ok(Recv::End) => Ok(Recv::End),
            Err(_) => Err(Error::Invalid),
        }
    }

    /// The `poll` events of endpoint `end`: `IN` with a message queued or
    /// a closed peer, `OUT` while the peer's queue has room or once the
    /// peer closed (a send fails at once), `HUP` once it closed.
    pub(crate) fn end_events(&self, end: u32) -> u32 {
        let queued = self.channels.queued(end).map_or(0, |(m, _)| m);
        if self.channels.is_open(end ^ 1) != Some(true) {
            return IN | OUT | HUP | RDHUP;
        }
        (if queued > 0 { IN } else { 0 })
            | if self.channels.has_room(end ^ 1) {
                OUT
            } else {
                0
            }
    }

    /// Grant `grant` was revoked, or left the table with its holder or
    /// object: its sockets read as closed from now on (reads 0, writes
    /// `EPIPE`), and its endpoint closes if no other socket holds it. The
    /// endpoints the kernel held through it, a cell handle, close: its
    /// observe streams end.
    pub fn grant_gone(&mut self, grant: u32) {
        let mut i = 0;
        while let Some(x) = self.held.get(i) {
            if x.grant == grant {
                self.unhold(i);
            } else {
                i += 1;
            }
        }
        for h in 0..self.sockets.len() as Handle {
            let Ok(&State::Endpoint {
                end,
                grant: g,
                next,
                ..
            }) = self.get(h).map(|s| &s.state)
            else {
                continue;
            };
            if g != grant {
                continue;
            }
            if let Ok(s) = self.get_mut(h) {
                s.state = State::Reset { pending: false };
                s.eof = true;
            }
            self.touch(h);
            self.leave_end(h, end, next);
        }
    }

    /// Whether grants wait to be dropped ([`take_dropped`](Self::take_dropped)).
    pub fn has_dropped(&self) -> bool {
        !self.dropped.is_empty()
    }

    /// The grants whose last socket closed since the last call: their
    /// holders let them go, and the kernel drops them from its table.
    pub fn take_dropped(&mut self) -> Vec<u32> {
        core::mem::take(&mut self.dropped)
    }

    /// Takes socket `h`, followed by `next`, off the list of endpoint
    /// `end`. With its last socket the endpoint closes: its peer's waiters
    /// are woken, and the handles queued on it are released (one level at
    /// a time, so nested messages never recurse deeper).
    pub(crate) fn leave_end(&mut self, h: Handle, end: u32, next: Handle) {
        let i = end as usize;
        if self.ends[i] == h {
            self.ends[i] = next;
        } else {
            let mut at = self.ends[i];
            while let Ok(Socket {
                state: State::Endpoint { next: n, .. },
                ..
            }) = self.get_mut(at)
            {
                if *n == h {
                    *n = next;
                    break;
                }
                at = *n;
            }
        }
        if self.ends[i] != NO_SOCKET {
            return;
        }
        self.close_end(end);
    }

    /// Closes endpoint `end`, which no socket holds any more: its peer's
    /// waiters are woken, the handles queued on it are released (one level
    /// at a time, so nested messages never recurse deeper), and a channel
    /// that ends gives its creator's room back ([`take_refunds`]).
    ///
    /// [`take_refunds`]: Self::take_refunds
    fn close_end(&mut self, end: u32) {
        let orphans = &mut self.orphans;
        if let Some(owner) = self.channels.close(end, &mut |x| orphans.push(x))
            && owner != rhizo_cap::HOST
        {
            self.refunds.push(owner);
            self.after = true;
        }
        self.touch_end(end ^ 1);
        self.drain_orphans();
        // The kernel's endpoint on the other side goes with its observer,
        // and with its input sender once what it sent was taken.
        if !self.held.is_empty()
            && let Some(i) = self.held.iter().position(|x| x.end == end ^ 1)
            && (!self.held[i].input || self.channels.peek(end ^ 1).is_none())
        {
            self.unhold(i);
        }
    }

    /// Cell handle `grant` was dropped (unlinked, or gone with its
    /// holder): what was opened through it stays open until its
    /// descriptors close, as a sender's close takes back nothing it sent
    /// (design D9), but no longer goes with the grant, whose number may
    /// name another one from now on.
    pub fn forget_grant(&mut self, grant: u32) {
        for x in self.held.iter_mut().filter(|x| x.grant == grant) {
            x.grant = u32::MAX;
        }
    }

    /// Releases the sockets of handles left in closed queues, unless a
    /// release up the stack is draining them already.
    #[inline]
    pub(crate) fn drain_orphans(&mut self) {
        if !self.orphans.is_empty() && !self.draining {
            self.draining = true;
            while let Some(x) = self.orphans.pop() {
                self.release(x);
            }
            self.draining = false;
        }
    }

    /// Ends channel `ch` where no socket holds its endpoints yet (making
    /// one failed half way): its room comes back to its creator
    /// ([`take_refunds`](Self::take_refunds)), as when any channel ends.
    pub fn drop_channel(&mut self, ch: u32) {
        for end in Channels::endpoints(ch) {
            let held = self.ends.get(end as usize).is_some_and(|&h| h != NO_SOCKET);
            if self.channels.is_open(end) == Some(true) && !held {
                self.close_end(end);
            }
        }
    }

    /// The inbox of cell `cell` (design D10): a listening sequenced-packet
    /// socket in its network, made at its first use and held by the kernel
    /// (one reference) until the cell ends ([`cell_ended`]); channels
    /// opened to the cell wait there to be accepted.
    ///
    /// [`cell_ended`]: Self::cell_ended
    pub fn inbox(&mut self, cell: u32) -> Result<Handle> {
        if let Some(&(_, h)) = self.inboxes.iter().find(|i| i.0 == cell) {
            return Ok(h);
        }
        self.inboxes.try_reserve(1).map_err(|_| Error::TooMany)?;
        let mut s = Socket::new(Family::Unix, Type::SeqPacket, Options::default(), cell);
        s.state = State::Listening {
            backlog: INBOX_MAX as usize,
            queue: VecDeque::new(),
        };
        let h = self.insert(s)?;
        self.inboxes.push((cell, h));
        Ok(h)
    }

    /// Queues endpoint socket `h` (its reference passes to the queue) on
    /// the inbox of cell `cell`, waking its waiters. `ConnRefused`,
    /// changing nothing, past [`INBOX_MAX`] channels waiting.
    pub fn deliver(&mut self, cell: u32, h: Handle) -> Result<()> {
        let inbox = self.inbox(cell)?;
        let State::Listening { backlog, queue } = &mut self.get_mut(inbox)?.state else {
            return Err(Error::Invalid);
        };
        if queue.len() >= *backlog {
            return Err(Error::ConnRefused);
        }
        queue.try_reserve(1).map_err(|_| Error::TooMany)?;
        queue.push_back(h);
        self.touch(inbox);
        Ok(())
    }

    /// The kernel holds endpoint `end` for cell `cell`, through the cell
    /// handle `grant`: it sends the cell's output there when `observe`
    /// ([`tell`](Self::tell)) and takes what arrives there as the cell's
    /// standard input when `input` ([`take_input`](Self::take_input)).
    /// What the peer sends to it never waits for the cell, nor what it
    /// sends for an observer: what does not fit is dropped, so nothing
    /// flows back to the cell (design D8).
    pub fn hold(
        &mut self,
        end: u32,
        cell: u32,
        grant: u32,
        observe: bool,
        input: bool,
    ) -> Result<()> {
        if self.channels.is_open(end) != Some(true) {
            return Err(Error::Invalid);
        }
        self.held.try_reserve(1).map_err(|_| Error::TooMany)?;
        self.held.push(Held {
            end,
            cell,
            grant,
            observe,
            input,
        });
        Ok(())
    }

    /// Whether the kernel holds an endpoint for some cell's observers.
    pub fn observed(&self) -> bool {
        self.held.iter().any(|x| x.observe)
    }

    /// Sends `bytes`, one message, to every observer of cell `cell` whose
    /// queue has room (the others miss it); an endpoint whose observer
    /// went is closed.
    pub fn tell(&mut self, cell: u32, bytes: &[u8]) {
        let mut i = 0;
        while let Some(&x) = self.held.get(i) {
            if x.cell != cell || !x.observe {
                i += 1;
                continue;
            }
            match self.channels.send(x.end, bytes, &[]) {
                Ok(()) => {
                    self.touch_end(x.end ^ 1);
                    i += 1;
                }
                Err(CapError::Pipe | CapError::NotFound) => self.unhold(i),
                Err(_) => i += 1,
            }
        }
    }

    /// The kernel lets go of held endpoint `held[i]`: its peer reads what
    /// is queued, then end of file.
    fn unhold(&mut self, i: usize) {
        let x = self.held.swap_remove(i);
        if self.channels.is_open(x.end) == Some(true) {
            self.close_end(x.end);
        }
    }

    /// Whether input waits for some cell ([`take_input`](Self::take_input)).
    pub fn has_input(&self) -> bool {
        !self.inputs.is_empty()
    }

    /// Whether input waits for cell `cell`.
    pub fn input_for(&self, cell: u32) -> bool {
        self.inputs.contains(&cell)
    }

    /// The cells input waits for.
    pub fn input_cells(&self) -> &[u32] {
        &self.inputs
    }

    /// Hands `push` the messages waiting as cell `cell`'s standard input,
    /// oldest first, each whole and only while it fits in `room` bytes.
    /// What is left waits for the next call. `push` returns `false` if the
    /// cell takes no input (none from the host, or closed): the endpoints
    /// held for its input then close, and their senders' writes fail with
    /// `EPIPE`.
    pub fn take_input(&mut self, cell: u32, mut room: usize, push: &mut dyn FnMut(&[u8]) -> bool) {
        let mut left = false;
        let mut buf = Vec::new();
        let mut i = 0;
        while let Some(&x) = self.held.get(i) {
            if x.cell != cell || !x.input {
                i += 1;
                continue;
            }
            let mut refused = false;
            while let Some(len) = self.channels.peek(x.end) {
                if len > room || buf.try_reserve(len).is_err() {
                    left = true;
                    break;
                }
                buf.clear();
                let mut hs = [0; MESSAGE_HANDLES_MAX as usize];
                let got = self.channels.recv_with(x.end, len, &mut hs, &mut |piece| {
                    buf.extend_from_slice(piece);
                    true
                });
                if got.is_err() {
                    break;
                }
                if !push(&buf) {
                    refused = true;
                    break;
                }
                room -= len;
                self.touch_end(x.end ^ 1);
            }
            // Refused, or its sender went and everything it sent was taken.
            let done = self.channels.is_open(x.end ^ 1) == Some(false)
                && self.channels.peek(x.end).is_none();
            if refused || done {
                self.unhold(i);
            } else {
                i += 1;
            }
        }
        if !left {
            self.inputs.retain(|&c| c != cell);
        }
    }

    /// Cell `cell` ended: `last` goes to each of its observers that has
    /// room before the endpoints the kernel held for it close (its
    /// observers read end of file once they read what was queued, its
    /// input senders' writes fail with `EPIPE`), its inbox closes with the
    /// channels waiting there, and the room of channels it made that ended
    /// is forgotten.
    pub fn cell_ended(&mut self, cell: u32, last: &[u8]) {
        self.tell(cell, last);
        let mut i = 0;
        while let Some(x) = self.held.get(i) {
            if x.cell == cell {
                self.unhold(i);
            } else {
                i += 1;
            }
        }
        self.inputs.retain(|&c| c != cell);
        if let Some(i) = self.inboxes.iter().position(|x| x.0 == cell) {
            let (_, h) = self.inboxes.swap_remove(i);
            self.release(h);
        }
        self.refunds.retain(|&c| c != cell);
    }

    /// How many channels cell `cell` made have ended since the last call:
    /// the room each reserved comes back to it ([`rhizo_cap::ROOM`]).
    pub fn take_refunds(&mut self, cell: u32) -> u32 {
        let before = self.refunds.len();
        self.refunds.retain(|&c| c != cell);
        (before - self.refunds.len()) as u32
    }

    /// Whether channels of some cell ended since it last took its refunds.
    pub fn has_refunds(&self) -> bool {
        !self.refunds.is_empty()
    }

    /// The calling cell received a message or a channel from another cell.
    pub(crate) fn received(&mut self) {
        self.received = true;
        self.after = true;
    }

    /// Whether the kernel has something to do after the call that holds
    /// the sockets: grants handed back ([`take_dropped`]), channel room to
    /// give back ([`take_refunds`]), a message or channel received
    /// ([`take_received`]). One test after each call that reached the
    /// sockets; it stays set while room waits for another cell.
    ///
    /// [`take_dropped`]: Self::take_dropped
    /// [`take_refunds`]: Self::take_refunds
    /// [`take_received`]: Self::take_received
    pub fn take_after(&mut self) -> bool {
        if !self.after {
            return false;
        }
        self.after = !self.refunds.is_empty();
        true
    }

    /// Whether the calling cell received a message or a channel from
    /// another cell since the last call: it may now hold a secret (its
    /// sticky bit, design D22 of `add-cell-hierarchy`).
    pub fn take_received(&mut self) -> bool {
        core::mem::take(&mut self.received)
    }
}
