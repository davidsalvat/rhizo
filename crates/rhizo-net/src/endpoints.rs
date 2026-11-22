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

use super::*;
use rhizo_cap::limits::MESSAGE_HANDLES_MAX;
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
        self.channels
            .send(end, data, handles)
            .map_err(|e| match e {
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
    /// `EPIPE`), and its endpoint closes if no other socket holds it.
    pub fn grant_gone(&mut self, grant: u32) {
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
        let orphans = &mut self.orphans;
        self.channels.close(end, &mut |x| orphans.push(x));
        self.touch_end(end ^ 1);
        if !self.draining {
            self.draining = true;
            while let Some(x) = self.orphans.pop() {
                self.release(x);
            }
            self.draining = false;
        }
    }
}
