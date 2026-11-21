//! A virtio socket device (virtio 1.2, section 5.10) and one stream over it
//! (change `control-over-vsock`): the host connects to the port the driver
//! listens on, and the stream carries bytes both ways under the device's
//! credit-based flow control.
//!
//! [`Stream`] is the protocol alone, with no device: it takes the packets
//! the device received, checks each as untrusted input, and says which to
//! send. It holds at most one connection at a time. [`Gate`] admits a
//! connection only once its first bytes are a secret (change
//! `authenticate-control-channel`). [`VirtioVsock`] moves packets between
//! the rings and the stream's owner.

use crate::{
    DESC_F_WRITE, DeviceRegisters, PhysicalMemory, ProbeError, QUEUE_SIZE, Queue, Transport,
    VSOCK_DEVICE, mmio, negotiate, pci, queue,
};
use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

/// Bytes of a packet header (`struct virtio_vsock_hdr`, little endian).
pub const HEADER: usize = 44;
/// The host's context identifier.
pub const HOST_CID: u64 = 2;
/// The socket type of streams, the only one served.
pub const STREAM: u16 = 1;
/// `VIRTIO_VSOCK_F_STREAM`.
const F_STREAM: u32 = 1;

/// Packet operations.
pub mod op {
    pub const REQUEST: u16 = 1;
    pub const RESPONSE: u16 = 2;
    pub const RST: u16 = 3;
    pub const SHUTDOWN: u16 = 4;
    pub const RW: u16 = 5;
    pub const CREDIT_UPDATE: u16 = 6;
    pub const CREDIT_REQUEST: u16 = 7;
}

/// A packet header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Header {
    pub src_cid: u64,
    pub dst_cid: u64,
    pub src_port: u32,
    pub dst_port: u32,
    /// Payload bytes.
    pub len: u32,
    /// Socket type ([`STREAM`]).
    pub kind: u16,
    pub op: u16,
    pub flags: u32,
    /// The sender's receive buffer, in bytes.
    pub buf_alloc: u32,
    /// Bytes the sender has taken from its receive buffer, ever (wrapping).
    pub fwd_cnt: u32,
}

impl Header {
    pub fn encode(&self) -> [u8; HEADER] {
        let mut b = [0u8; HEADER];
        b[0..8].copy_from_slice(&self.src_cid.to_le_bytes());
        b[8..16].copy_from_slice(&self.dst_cid.to_le_bytes());
        b[16..20].copy_from_slice(&self.src_port.to_le_bytes());
        b[20..24].copy_from_slice(&self.dst_port.to_le_bytes());
        b[24..28].copy_from_slice(&self.len.to_le_bytes());
        b[28..30].copy_from_slice(&self.kind.to_le_bytes());
        b[30..32].copy_from_slice(&self.op.to_le_bytes());
        b[32..36].copy_from_slice(&self.flags.to_le_bytes());
        b[36..40].copy_from_slice(&self.buf_alloc.to_le_bytes());
        b[40..44].copy_from_slice(&self.fwd_cnt.to_le_bytes());
        b
    }

    pub fn decode(b: &[u8; HEADER]) -> Self {
        let u64_at = |at: usize| u64::from_le_bytes(core::array::from_fn(|i| b[at + i]));
        let u32_at = |at: usize| u32::from_le_bytes(core::array::from_fn(|i| b[at + i]));
        let u16_at = |at: usize| u16::from_le_bytes([b[at], b[at + 1]]);
        Self {
            src_cid: u64_at(0),
            dst_cid: u64_at(8),
            src_port: u32_at(16),
            dst_port: u32_at(20),
            len: u32_at(24),
            kind: u16_at(28),
            op: u16_at(30),
            flags: u32_at(32),
            buf_alloc: u32_at(36),
            fwd_cnt: u32_at(40),
        }
    }
}

/// Bytes the peer may send that the stream has not read yet: the receive
/// buffer it advertises.
pub const BUF_ALLOC: u32 = 256 * 1024;
/// Bytes written and not yet sent beyond which the connection is reset: a
/// peer that stops reading does not get to hold the kernel's memory.
pub const TX_MAX: usize = 4 << 20;
/// Control packets (responses, resets, credit) kept to send; more are
/// dropped.
const CONTROL_MAX: usize = 64;

/// The connected peer, and the credit it last gave.
#[derive(Debug, Clone, Copy)]
struct Peer {
    cid: u64,
    port: u32,
    buf_alloc: u32,
    fwd_cnt: u32,
}

/// What a packet changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// A peer connected.
    Connected,
    /// The connection ended (the peer closed it, or broke the protocol).
    Closed,
}

/// One stream socket listening on a port.
#[derive(Debug)]
pub struct Stream {
    cid: u64,
    port: u32,
    peer: Option<Peer>,
    /// Received, not yet read.
    rx: VecDeque<u8>,
    /// Written, not yet sent.
    tx: VecDeque<u8>,
    /// Payload bytes sent, and read, on this connection (wrapping).
    tx_cnt: u32,
    fwd_cnt: u32,
    /// `fwd_cnt` as the peer last heard it.
    told: u32,
    /// A credit request went out since the peer last gave credit.
    asked: bool,
    control: VecDeque<Header>,
    /// Connections made, ever (wrapping): the current one's number.
    made: u32,
}

impl Stream {
    /// A stream listening on `port` of context `cid` (the device's).
    pub fn new(cid: u64, port: u32) -> Self {
        Self {
            cid,
            port,
            peer: None,
            rx: VecDeque::new(),
            tx: VecDeque::new(),
            tx_cnt: 0,
            fwd_cnt: 0,
            told: 0,
            asked: false,
            control: VecDeque::new(),
            made: 0,
        }
    }

    /// Whether a peer is connected.
    pub fn connected(&self) -> bool {
        self.peer.is_some()
    }

    /// The number of the connection a peer holds, if any: every connection
    /// gets a new one, so an owner that compares numbers sees a new
    /// connection even when the old one ended and the new one began between
    /// two of its looks.
    pub fn connection(&self) -> Option<u32> {
        self.peer.map(|_| self.made)
    }

    /// Takes a packet the device received, with the payload bytes it holds.
    pub fn take(&mut self, h: &Header, data: &[u8]) -> Option<Change> {
        let ours = h.dst_cid == self.cid && h.dst_port == self.port;
        let from_peer = ours
            && self
                .peer
                .is_some_and(|p| p.cid == h.src_cid && p.port == h.src_port);
        if !from_peer {
            let request = h.op == op::REQUEST && h.kind == STREAM && h.len == 0;
            if ours && request && data.is_empty() && self.peer.is_none() {
                self.peer = Some(Peer {
                    cid: h.src_cid,
                    port: h.src_port,
                    buf_alloc: h.buf_alloc,
                    fwd_cnt: h.fwd_cnt,
                });
                (self.tx_cnt, self.fwd_cnt, self.told, self.asked) = (0, 0, 0, false);
                self.made = self.made.wrapping_add(1);
                self.reply(h, op::RESPONSE);
                return Some(Change::Connected);
            }
            // No connection here: say so, except to a reset.
            if h.op != op::RST {
                self.reply(h, op::RST);
            }
            return None;
        }
        if let Some(p) = self.peer.as_mut() {
            (p.buf_alloc, p.fwd_cnt) = (h.buf_alloc, h.fwd_cnt);
        }
        if h.kind != STREAM || h.len as usize != data.len() {
            return Some(self.reset());
        }
        match h.op {
            op::RW => {
                // More than the credit it was given.
                if self.rx.len() + data.len() > BUF_ALLOC as usize {
                    return Some(self.reset());
                }
                self.rx.extend(data);
            }
            op::CREDIT_REQUEST => self.reply(h, op::CREDIT_UPDATE),
            op::SHUTDOWN => return Some(self.reset()),
            op::RST => {
                self.close();
                return Some(Change::Closed);
            }
            // Credit updates were taken above; anything else is ignored.
            _ => {}
        }
        if self.credit() > 0 {
            self.asked = false;
        }
        None
    }

    /// Whether bytes written or control packets still wait to go out to a
    /// connected peer (then the kernel must not power off yet).
    pub fn sending(&self) -> bool {
        !self.control.is_empty() || (self.peer.is_some() && !self.tx.is_empty())
    }

    /// Whether bytes received wait to be read.
    pub fn has_input(&self) -> bool {
        !self.rx.is_empty()
    }

    /// The next byte received on connection `connection` (its
    /// [`number`](Self::connection)), if any: none once another connection
    /// took its place, so no byte of a new connection is read as the old
    /// one's.
    pub fn read(&mut self, connection: u32) -> Option<u8> {
        if self.connection() != Some(connection) {
            return None;
        }
        let byte = self.rx.pop_front()?;
        self.fwd_cnt = self.fwd_cnt.wrapping_add(1);
        Some(byte)
    }

    /// Queues `bytes` to send; `false` if no peer is connected or it reads
    /// too little to hold them (the connection is then reset).
    pub fn write(&mut self, bytes: &[u8]) -> bool {
        if self.peer.is_none() {
            return false;
        }
        if self.tx.len() + bytes.len() > TX_MAX {
            self.reset();
            return false;
        }
        self.tx.extend(bytes);
        true
    }

    /// The next packet to send, its payload (at most `max` bytes) in `out`.
    pub fn next_packet(&mut self, max: usize, out: &mut Vec<u8>) -> Option<Header> {
        out.clear();
        if let Some(mut h) = self.control.pop_front() {
            h.fwd_cnt = self.fwd_cnt;
            if self
                .peer
                .is_some_and(|p| p.cid == h.dst_cid && p.port == h.dst_port)
            {
                self.told = self.fwd_cnt;
            }
            return Some(h);
        }
        self.peer?;
        if !self.tx.is_empty() {
            let n = self.tx.len().min(max).min(self.credit() as usize);
            if n > 0 {
                out.extend(self.tx.drain(..n));
                self.tx_cnt = self.tx_cnt.wrapping_add(n as u32);
                return self.for_peer(op::RW, n as u32);
            }
            if !self.asked {
                self.asked = true;
                return self.for_peer(op::CREDIT_REQUEST, 0);
            }
        }
        if self.fwd_cnt.wrapping_sub(self.told) >= BUF_ALLOC / 2 {
            return self.for_peer(op::CREDIT_UPDATE, 0);
        }
        None
    }

    /// Bytes the peer can take now.
    fn credit(&self) -> u32 {
        self.peer.map_or(0, |p| {
            p.buf_alloc
                .saturating_sub(self.tx_cnt.wrapping_sub(p.fwd_cnt))
        })
    }

    /// A packet to the peer, telling it what was read.
    fn for_peer(&mut self, op: u16, len: u32) -> Option<Header> {
        let p = self.peer?;
        self.told = self.fwd_cnt;
        Some(self.header(p.cid, p.port, self.port, op, len))
    }

    fn header(&self, cid: u64, port: u32, src_port: u32, op: u16, len: u32) -> Header {
        Header {
            src_cid: self.cid,
            dst_cid: cid,
            src_port,
            dst_port: port,
            len,
            kind: STREAM,
            op,
            flags: 0,
            buf_alloc: BUF_ALLOC,
            fwd_cnt: self.fwd_cnt,
        }
    }

    /// Answers packet `h` with an empty packet `op`.
    fn reply(&mut self, h: &Header, op: u16) {
        self.push(self.header(h.src_cid, h.src_port, h.dst_port, op, 0));
    }

    fn push(&mut self, h: Header) {
        if self.control.len() < CONTROL_MAX {
            self.control.push_back(h);
        }
    }

    /// The address of the peer that holds the connection, if any.
    pub fn peer(&self) -> Option<(u64, u32)> {
        self.peer.map(|p| (p.cid, p.port))
    }

    /// Resets the connection.
    pub fn reset(&mut self) -> Change {
        if let Some(p) = self.peer {
            self.push(self.header(p.cid, p.port, self.port, op::RST, 0));
        }
        self.close();
        Change::Closed
    }

    fn close(&mut self) {
        self.peer = None;
        self.rx.clear();
        self.tx.clear();
    }
}

/// Bytes of the secret a connection presents first.
pub const SECRET_LEN: usize = 32;
/// How long a connection may take to present the secret, from the first
/// [`Gate::expire`] that sees it.
pub const PROVE_WITHIN_NS: u64 = 2_000_000_000;

/// A [`Stream`] that admits a connection only once the first
/// [`SECRET_LEN`] bytes it sends are the secret (change
/// `authenticate-control-channel`). Until then nothing it sends is read
/// and nothing is written to it. A wrong secret, one not given within
/// [`PROVE_WITHIN_NS`], or another peer's request resets it, and the
/// stream keeps listening. An admitted connection is held as by the stream
/// alone.
#[derive(Debug)]
pub struct Gate {
    stream: Stream,
    secret: [u8; SECRET_LEN],
    /// The number of the connection admitted.
    admitted: Option<u32>,
    /// The connection presenting the secret: its number, and when
    /// [`expire`](Self::expire) first saw it.
    proving: Option<(u32, Option<u64>)>,
    proof: [u8; SECRET_LEN],
    have: usize,
    refused: u32,
}

impl Gate {
    /// A gate on `stream`, which admits connections that present `secret`.
    pub fn new(stream: Stream, secret: [u8; SECRET_LEN]) -> Self {
        Self {
            stream,
            secret,
            admitted: None,
            proving: None,
            proof: [0; SECRET_LEN],
            have: 0,
            refused: 0,
        }
    }

    /// Takes a packet the device received (as [`Stream::take`]); a
    /// connection is [`Connected`](Change::Connected) once admitted. A
    /// request from another peer replaces a connection that has not
    /// presented the secret, so none holds the only connection without it.
    pub fn take(&mut self, h: &Header, data: &[u8]) -> Option<Change> {
        let before = self.connection();
        let ours = (h.dst_cid, h.dst_port) == (self.stream.cid, self.stream.port);
        if h.op == op::REQUEST
            && ours
            && before.is_none()
            && self
                .stream
                .peer()
                .is_some_and(|p| p != (h.src_cid, h.src_port))
        {
            self.refuse();
        }
        let change = self.stream.take(h, data);
        self.prove();
        let after = self.connection();
        if after.is_some() && after != before {
            Some(Change::Connected)
        } else if before.is_some() && change == Some(Change::Closed) {
            change
        } else {
            None
        }
    }

    /// Reads the secret's bytes from the connection presenting it, and
    /// admits or refuses it once all have come: never before, so a wrong
    /// byte tells nothing about where it is.
    fn prove(&mut self) {
        let Some(c) = self.stream.connection() else {
            (self.admitted, self.proving) = (None, None);
            return;
        };
        if self.admitted == Some(c) {
            return;
        }
        if self.proving.is_none_or(|(p, _)| p != c) {
            (self.admitted, self.proving, self.have) = (None, Some((c, None)), 0);
        }
        while self.have < SECRET_LEN {
            let Some(byte) = self.stream.read(c) else {
                return;
            };
            self.proof[self.have] = byte;
            self.have += 1;
        }
        if same(&self.proof, &self.secret) {
            (self.admitted, self.proving) = (Some(c), None);
        } else {
            self.refuse();
        }
    }

    /// Resets the connection presenting the secret, and counts it.
    fn refuse(&mut self) {
        self.stream.reset();
        self.proving = None;
        self.refused = self.refused.wrapping_add(1);
    }

    /// Resets the connection presenting the secret if it has taken
    /// [`PROVE_WITHIN_NS`] since the first call that saw it; `now` is a
    /// monotonic time in nanoseconds.
    pub fn expire(&mut self, now: u64) {
        match &mut self.proving {
            Some((_, since @ None)) => *since = Some(now),
            Some((_, Some(since))) if now.wrapping_sub(*since) >= PROVE_WITHIN_NS => {
                self.refuse();
            }
            _ => {}
        }
    }

    /// Connections refused, ever (wrapping).
    pub fn refused(&self) -> u32 {
        self.refused
    }

    /// The number of the connection admitted, while it is held.
    pub fn connection(&self) -> Option<u32> {
        self.admitted
            .filter(|&c| self.stream.connection() == Some(c))
    }

    /// The next byte the admitted connection `connection` sent (as
    /// [`Stream::read`]).
    pub fn read(&mut self, connection: u32) -> Option<u8> {
        if self.connection() != Some(connection) {
            return None;
        }
        self.stream.read(connection)
    }

    /// Whether bytes of the admitted connection wait to be read.
    pub fn has_input(&self) -> bool {
        self.connection().is_some() && self.stream.has_input()
    }

    /// Queues `bytes` for the admitted connection (as [`Stream::write`]);
    /// `false` if none is admitted.
    pub fn write(&mut self, bytes: &[u8]) -> bool {
        self.connection().is_some() && self.stream.write(bytes)
    }

    /// As [`Stream::next_packet`].
    pub fn next_packet(&mut self, max: usize, out: &mut Vec<u8>) -> Option<Header> {
        self.stream.next_packet(max, out)
    }

    /// As [`Stream::sending`].
    pub fn sending(&self) -> bool {
        self.stream.sending()
    }
}

/// Whether `a` and `b` are equal, in a time that does not depend on where
/// they differ.
fn same(a: &[u8; SECRET_LEN], b: &[u8; SECRET_LEN]) -> bool {
    let diff = a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y));
    core::hint::black_box(diff) == 0
}

/// Packet slots per queue: a header descriptor chained to a payload one.
const SLOTS: u16 = QUEUE_SIZE / 2;
/// Most payload bytes in one packet.
pub const PAYLOAD_MAX: usize = crate::BUF as usize;
/// Frames [`VirtioVsock::probe`] takes: each of its three queues' ring and
/// buffers.
pub const FRAMES: usize = crate::FRAMES / 2 * 3;

/// A virtio socket device.
#[derive(Debug)]
pub struct VirtioVsock<R, M> {
    regs: Transport<R>,
    mem: M,
    cid: u64,
    rx: Queue,
    tx: Queue,
    /// Transmit slots (their header descriptors) the device gave back.
    tx_free: Vec<u16>,
    payload: Vec<u8>,
}

impl<R: DeviceRegisters, M: PhysicalMemory> VirtioVsock<R, M> {
    /// Brings up the socket device behind virtio-mmio registers `regs`,
    /// taking frames for its rings and buffers from `alloc` (never
    /// returned: the device lives as long as the kernel).
    pub fn probe(
        regs: R,
        mem: M,
        alloc: &mut dyn FnMut() -> Option<u64>,
    ) -> Result<Self, ProbeError> {
        Self::start(mmio(regs, VSOCK_DEVICE)?, mem, alloc)
    }

    /// Brings up the modern virtio-pci socket device whose structures
    /// `transport` reaches (see [`pci::regions`]).
    pub fn probe_pci(
        transport: pci::Transport<R>,
        mem: M,
        alloc: &mut dyn FnMut() -> Option<u64>,
    ) -> Result<Self, ProbeError> {
        Self::start(Transport::Pci(transport), mem, alloc)
    }

    fn start(
        mut regs: Transport<R>,
        mut mem: M,
        alloc: &mut dyn FnMut() -> Option<u64>,
    ) -> Result<Self, ProbeError> {
        negotiate(&mut regs, F_STREAM)?;
        let cid = u64::from(regs.config32(0)) | (u64::from(regs.config32(4)) << 32);
        let rx = queue(&mut regs, &mut mem, alloc, 0)?;
        let tx = queue(&mut regs, &mut mem, alloc, 1)?;
        // Events (a transport reset after the VM was restored) are not
        // taken: no buffer is offered on that queue.
        queue(&mut regs, &mut mem, alloc, 2)?;
        let mut dev = Self {
            regs,
            mem,
            cid,
            rx,
            tx,
            tx_free: (0..SLOTS).rev().map(|s| 2 * s).collect(),
            payload: vec![0; PAYLOAD_MAX],
        };
        for head in (0..SLOTS).map(|s| 2 * s) {
            let rx = &dev.rx;
            rx.describe(&mut dev.mem, head, HEADER as u32, DESC_F_WRITE, true);
            rx.describe(
                &mut dev.mem,
                head + 1,
                PAYLOAD_MAX as u32,
                DESC_F_WRITE,
                false,
            );
            dev.rx.offer(&mut dev.mem, head);
        }
        dev.regs
            .set_status(crate::ACKNOWLEDGE | crate::DRIVER | crate::FEATURES_OK | crate::DRIVER_OK);
        dev.rx.publish(&mut dev.mem, &mut dev.regs);
        Ok(dev)
    }

    /// This VM's context identifier.
    pub fn cid(&self) -> u64 {
        self.cid
    }

    /// Hands the next packet the device received to `take`, with the
    /// payload bytes it holds; `false` if there is none.
    pub fn recv(&mut self, take: &mut dyn FnMut(&Header, &[u8])) -> bool {
        self.regs.barrier();
        let Some((id, len)) = self.rx.take_used(&self.mem) else {
            return false;
        };
        let head = id & !1;
        let mut h = [0u8; HEADER];
        self.mem.read_bytes(self.rx.bufs[head as usize], &mut h);
        let header = Header::decode(&h);
        let n = (len as usize)
            .saturating_sub(HEADER)
            .min(header.len as usize)
            .min(PAYLOAD_MAX);
        let payload = &mut self.payload[..n];
        self.mem
            .read_bytes(self.rx.bufs[head as usize + 1], payload);
        // The slot goes back to the device before the packet is handed on.
        self.rx.offer(&mut self.mem, head);
        self.rx.publish(&mut self.mem, &mut self.regs);
        if len as usize >= HEADER {
            take(&header, &self.payload[..n]);
        }
        true
    }

    /// Whether a transmit slot is free.
    pub fn can_send(&mut self) -> bool {
        self.reclaim();
        !self.tx_free.is_empty()
    }

    /// Whether the device is done with every packet sent: none is still in
    /// a transmit slot.
    pub fn sent_all(&mut self) -> bool {
        self.reclaim();
        self.tx_free.len() == usize::from(SLOTS)
    }

    /// Takes back the transmit slots the device is done with.
    fn reclaim(&mut self) {
        self.regs.barrier();
        while let Some((id, _)) = self.tx.take_used(&self.mem) {
            let head = id & !1;
            if !self.tx_free.contains(&head) {
                self.tx_free.push(head);
            }
        }
    }

    /// Sends packet `h` with payload `data` (`h.len` bytes, at most
    /// [`PAYLOAD_MAX`]); `false` if no transmit slot is free.
    pub fn send(&mut self, h: &Header, data: &[u8]) -> bool {
        self.reclaim();
        if data.len() > PAYLOAD_MAX {
            return false;
        }
        let Some(head) = self.tx_free.pop() else {
            return false;
        };
        let tx = &self.tx;
        self.mem.write_bytes(tx.bufs[head as usize], &h.encode());
        tx.describe(&mut self.mem, head, HEADER as u32, 0, !data.is_empty());
        if !data.is_empty() {
            self.mem.write_bytes(tx.bufs[head as usize + 1], data);
            tx.describe(&mut self.mem, head + 1, data.len() as u32, 0, false);
        }
        self.tx.offer(&mut self.mem, head);
        self.tx.publish(&mut self.mem, &mut self.regs);
        true
    }
}

#[cfg(test)]
mod tests;
