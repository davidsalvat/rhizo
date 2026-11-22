//! Channels (design D10 of `add-cell-hierarchy`): two endpoints, each with
//! a receive queue bounded to [`QUEUE_MESSAGES_MAX`] messages and
//! [`QUEUE_BYTES_MAX`] bytes. A message holds at most [`MESSAGE_MAX`] bytes
//! and [`MESSAGE_HANDLES_MAX`] handles; the kernel copies bytes in and out
//! and no memory is ever shared.
//!
//! The room for both queues ([`ROOM`]) is reserved from the creator's
//! memory budget when the channel is made, so a send into a full queue
//! fails with [`Error::Full`] (`EAGAIN`, or the sender blocks), never
//! because of anyone's budget. Queues take heap memory only as messages
//! wait: the reservation is accounting, and the admission of reserved
//! roots (design D4) keeps that memory for them.
//!
//! Who may use an endpoint is the grant table's business ([`Grants`]):
//! here an endpoint is a number, `2 * channel + side`.
//!
//! [`Grants`]: crate::Grants

use crate::limits::{MESSAGE_HANDLES_MAX, MESSAGE_MAX, QUEUE_BYTES_MAX, QUEUE_MESSAGES_MAX};
use crate::tree::Usage;
use crate::{Actor, Error, HOST, Tree, alloc_point, reserve};
use alloc::collections::VecDeque;
use alloc::vec::Vec;

/// One queued message: its length and how many handles it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Meta {
    len: u32,
    handles: u32,
}

/// Most handles one queue holds.
const QUEUE_HANDLES_MAX: usize = (QUEUE_MESSAGES_MAX * MESSAGE_HANDLES_MAX) as usize;

/// The memory a channel reserves from its creator: both queues full, their
/// bytes with the records of their messages and handles.
pub const ROOM: u64 = 2
    * (QUEUE_BYTES_MAX as u64
        + QUEUE_MESSAGES_MAX as u64 * core::mem::size_of::<Meta>() as u64
        + QUEUE_HANDLES_MAX as u64 * 4);

/// The messages one endpoint receives, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Queue {
    bytes: VecDeque<u8>,
    messages: VecDeque<Meta>,
    handles: VecDeque<u32>,
}

/// Room in `q` for `n` more items, growing it by doubling but never past
/// `max` items, or [`Error::NoMemory`] with `q` unchanged.
fn grow<T>(q: &mut VecDeque<T>, n: usize, max: usize) -> Result<(), Error> {
    if q.len() + n <= q.capacity() {
        return Ok(());
    }
    let want = (q.capacity() * 2).max(q.len() + n).min(max);
    alloc_point()?;
    q.try_reserve_exact(want - q.len())
        .map_err(|_| Error::NoMemory)
}

/// A channel: its creator, charged its [`ROOM`] ([`HOST`] for the host),
/// the queue of each endpoint, and which endpoints are still open.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Channel {
    owner: u32,
    queues: [Queue; 2],
    open: [bool; 2],
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Slot {
    Used(Channel),
    /// A free slot, and the next free one.
    Free(u32),
}

/// No channel.
const NIL: u32 = u32::MAX;

/// What a receive found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recv {
    /// The oldest message: its full length (`MSG_TRUNC`: the buffer got
    /// what fits, the rest is dropped) and the number of handles written.
    Message { len: usize, handles: usize },
    /// Nothing queued (`EAGAIN`, or the receiver blocks).
    Empty,
    /// Nothing queued and the peer closed: end of file.
    End,
}

/// The kernel's channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channels {
    slots: Vec<Slot>,
    free: u32,
}

impl Default for Channels {
    fn default() -> Self {
        Self::new()
    }
}

impl Channels {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: NIL,
        }
    }

    fn get(&self, ch: u32) -> Option<&Channel> {
        match self.slots.get(ch as usize) {
            Some(Slot::Used(c)) => Some(c),
            _ => None,
        }
    }

    fn get_mut(&mut self, ch: u32) -> Option<&mut Channel> {
        match self.slots.get_mut(ch as usize) {
            Some(Slot::Used(c)) => Some(c),
            _ => None,
        }
    }

    /// The creator of channel `ch` ([`HOST`] for the host), if it lives.
    pub fn owner(&self, ch: u32) -> Option<u32> {
        self.get(ch).map(|c| c.owner)
    }

    /// The two endpoints of channel `ch`.
    pub fn endpoints(ch: u32) -> [u32; 2] {
        [2 * ch, 2 * ch + 1]
    }

    /// Makes a channel for `by`, both endpoints open and empty, and
    /// returns its number. A cell must have [`ROOM`] free in its memory
    /// budget (it uses `usage(cell)` itself), which the kernel then charges
    /// to it until the channel ends; the host's channels are charged to
    /// nobody.
    pub fn create(&mut self, tree: &Tree, by: Actor, usage: Usage) -> Result<u32, Error> {
        let owner = match by {
            Actor::Host => HOST,
            Actor::Cell(c) => {
                let free = tree.free_budget(c, usage(c)).ok_or(Error::NotFound)?;
                if free.memory < ROOM {
                    return Err(Error::Budget);
                }
                c
            }
        };
        let channel = Channel {
            owner,
            queues: Default::default(),
            open: [true; 2],
        };
        if self.free != NIL {
            let ch = self.free;
            let Slot::Free(next) = self.slots[ch as usize] else {
                unreachable!("the free list holds free slots");
            };
            self.free = next;
            self.slots[ch as usize] = Slot::Used(channel);
            return Ok(ch);
        }
        // Endpoint numbers stay within `u32`.
        if self.slots.len() >= (NIL >> 1) as usize {
            return Err(Error::NoMemory);
        }
        reserve(&mut self.slots, 1)?;
        self.slots.push(Slot::Used(channel));
        Ok((self.slots.len() - 1) as u32)
    }

    /// Sends one message from endpoint `end` to its peer: `bytes` and the
    /// handles `handles` (numbers the kernel gives the handles in flight).
    /// Refused, changing nothing: over [`MESSAGE_MAX`] bytes
    /// ([`Error::TooBig`]) or [`MESSAGE_HANDLES_MAX`] handles
    /// ([`Error::TooManyRefs`]), to a closed peer ([`Error::Pipe`]), or
    /// into a peer's queue that cannot take it ([`Error::Full`]).
    pub fn send(&mut self, end: u32, bytes: &[u8], handles: &[u32]) -> Result<(), Error> {
        let c = self.get_mut(end >> 1).ok_or(Error::NotFound)?;
        let side = (end & 1) as usize;
        if !c.open[side] {
            return Err(Error::NotFound);
        }
        if bytes.len() > MESSAGE_MAX as usize {
            return Err(Error::TooBig);
        }
        if handles.len() > MESSAGE_HANDLES_MAX as usize {
            return Err(Error::TooManyRefs);
        }
        if !c.open[side ^ 1] {
            return Err(Error::Pipe);
        }
        let q = &mut c.queues[side ^ 1];
        if q.messages.len() >= QUEUE_MESSAGES_MAX as usize
            || q.bytes.len() + bytes.len() > QUEUE_BYTES_MAX as usize
        {
            return Err(Error::Full);
        }
        grow(&mut q.bytes, bytes.len(), QUEUE_BYTES_MAX as usize)?;
        grow(&mut q.messages, 1, QUEUE_MESSAGES_MAX as usize)?;
        grow(&mut q.handles, handles.len(), QUEUE_HANDLES_MAX)?;
        // Nothing fails from here.
        q.bytes.extend(bytes);
        q.handles.extend(handles);
        q.messages.push_back(Meta {
            len: bytes.len() as u32,
            handles: handles.len() as u32,
        });
        Ok(())
    }

    /// Receives the oldest message queued on endpoint `end`: as much of it
    /// as fits in `buf`, and its handles into `handles`.
    pub fn recv(
        &mut self,
        end: u32,
        buf: &mut [u8],
        handles: &mut [u32; MESSAGE_HANDLES_MAX as usize],
    ) -> Result<Recv, Error> {
        let c = self.get_mut(end >> 1).ok_or(Error::NotFound)?;
        let side = (end & 1) as usize;
        if !c.open[side] {
            return Err(Error::NotFound);
        }
        let peer_open = c.open[side ^ 1];
        let q = &mut c.queues[side];
        let Some(m) = q.messages.pop_front() else {
            return Ok(if peer_open { Recv::Empty } else { Recv::End });
        };
        let (len, h) = (m.len as usize, m.handles as usize);
        let n = len.min(buf.len());
        let (a, b) = q.bytes.as_slices();
        let k = n.min(a.len());
        buf[..k].copy_from_slice(&a[..k]);
        buf[k..n].copy_from_slice(&b[..n - k]);
        q.bytes.drain(..len);
        for (out, v) in handles.iter_mut().zip(q.handles.drain(..h)) {
            *out = v;
        }
        Ok(Recv::Message { len, handles: h })
    }

    /// The number of messages and bytes queued on endpoint `end`.
    pub fn queued(&self, end: u32) -> Option<(usize, usize)> {
        let q = &self.get(end >> 1)?.queues[(end & 1) as usize];
        Some((q.messages.len(), q.bytes.len()))
    }

    /// Closes endpoint `end` (its last holder let it go): the messages
    /// queued on it are dropped, each handle they carried told to `gone`;
    /// its peer reads what is queued for it, then end of file, and its
    /// sends fail with [`Error::Pipe`]. Once both endpoints are closed the
    /// channel ends, and its creator, whose [`ROOM`] comes back, is
    /// returned.
    pub fn close(&mut self, end: u32, gone: &mut dyn FnMut(u32)) -> Option<u32> {
        let c = self.get_mut(end >> 1)?;
        let side = (end & 1) as usize;
        if !c.open[side] {
            return None;
        }
        c.open[side] = false;
        let q = core::mem::take(&mut c.queues[side]);
        q.handles.into_iter().for_each(&mut *gone);
        if c.open[side ^ 1] {
            return None;
        }
        let owner = c.owner;
        self.slots[(end >> 1) as usize] = Slot::Free(self.free);
        self.free = end >> 1;
        Some(owner)
    }
}
