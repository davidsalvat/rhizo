//! The control protocol between the host and a control kernel (spec
//! `control-plane`, design D2 of `add-control-plane`): the host asks the
//! kernel to run, list, suspend, resume, fork and kill cells, to freeze
//! them into templates, to save and load snapshots as bytes (change
//! `snapshots-on-disk`), to place and move cells in the cell tree and to
//! publish templates (version 4, design D14 and D21 of
//! `add-cell-hierarchy`); the kernel answers each
//! request with its outcome and the time it took.
//!
//! Both directions carry the same frames on one byte stream, which may also
//! carry plain text (the kernel's console, on a serial line it shares):
//!
//! ```text
//! 0x01 'Z' version kind id:u32 length:u32 payload[length]
//! ```
//!
//! Numbers are little endian. Text never holds the byte 0x01, so a
//! [`Decoder`] tells frames from text, and finds the next frame after a
//! malformed one. A request's identifier comes back in its reply, and in
//! the output events of an `output` request. Everything read is checked:
//! decoding never panics and never allocates more than the bytes it was
//! given (design D3 of `add-control-plane`: this is an attack surface).
//!
//! The kernel speaks [`VERSION`] only. The host speaks it and [`PREVIOUS`]
//! (design D20 of `add-cell-hierarchy`), so a newer `rhizo` still drives a
//! VM booted before an upgrade: [`Request::encode_as`] writes a request in
//! either, and decoding follows the version of the frame.
//!
//! The kernel also sends the host audit records, unasked, with identifier
//! 0 ([`Message::Audit`], design D18 of `add-cell-hierarchy`); [`audit`]
//! has the records and the ring that keeps them until they leave.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod audit;

use alloc::vec::Vec;

/// The protocol's version, in every frame (2: standard input, design D2 of
/// `add-interactive-chat`; 3: templates, save and load, design D2 of
/// `snapshots-on-disk`; 4: the cell tree, design D14 of
/// `add-cell-hierarchy`).
pub const VERSION: u8 = 4;
/// The version before, which the host still speaks (design D20 of
/// `add-cell-hierarchy`).
pub const PREVIOUS: u8 = 3;
/// The bytes every frame starts with.
pub const MAGIC: [u8; 2] = [0x01, b'Z'];
/// Bytes of a frame before its payload.
pub const HEADER_LEN: usize = 12;
/// Largest payload a frame may carry.
pub const MAX_PAYLOAD: usize = 1 << 20;
/// Most bytes one `input` request may carry: what a cell's standard input
/// holds (a pipe of 64 KiB), so an accepted request always fits an empty
/// one.
pub const MAX_INPUT: usize = 64 << 10;
/// The virtio-vsock port a control kernel listens on (change
/// `control-over-vsock`): the host connects there, and the stream carries
/// the same frames as the serial line, without console text.
pub const VSOCK_PORT: u32 = 1024;
/// The console line, on the serial line before the hello, that gives the
/// host the secret every vsock connection presents before its frames
/// (change `authenticate-control-channel`): this prefix, then the
/// [`SECRET_LEN`] bytes in lowercase hex.
pub const SECRET_LINE: &str = "vsock: secret ";
/// Bytes of that secret.
pub const SECRET_LEN: usize = 32;

/// Frame kinds of requests, host to kernel.
mod kind {
    pub const RUN: u8 = 0x01;
    pub const LIST: u8 = 0x02;
    pub const SUSPEND: u8 = 0x03;
    pub const RESUME: u8 = 0x04;
    pub const FORK: u8 = 0x05;
    pub const KILL: u8 = 0x06;
    pub const OUTPUT: u8 = 0x07;
    pub const WRITE_FILE: u8 = 0x08;
    pub const STATS: u8 = 0x09;
    pub const SHUTDOWN: u8 = 0x0a;
    pub const INPUT: u8 = 0x0b;
    pub const CLOSE_INPUT: u8 = 0x0c;
    pub const FREEZE: u8 = 0x0d;
    pub const SAVE: u8 = 0x0e;
    pub const LOAD: u8 = 0x0f;
    pub const REPARENT: u8 = 0x10;
    pub const POLICY: u8 = 0x11;
    pub const LIMITS: u8 = 0x12;
    pub const SCOPED: u8 = 0x13;
    pub const FLOOR: u8 = 0x14;
    pub const PUBLISH: u8 = 0x15;
    pub const WITHDRAW: u8 = 0x16;
    // Kernel to host.
    pub const HELLO: u8 = 0x80;
    pub const DONE: u8 = 0x81;
    pub const FAILED: u8 = 0x82;
    pub const OUTPUT_DATA: u8 = 0x83;
    pub const ENDED: u8 = 0x84;
    pub const AUDIT: u8 = 0x85;
}

/// Why a frame or a request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The payload does not decode as its kind says.
    Malformed,
    /// The frame has another protocol version.
    Version,
    /// The payload is larger than [`MAX_PAYLOAD`], or memory cannot hold it.
    TooLarge,
    /// No request or message has this kind.
    UnknownKind,
    /// No cell has this identifier.
    NoCell,
    /// The cell is not running.
    NotRunning,
    /// The cell is not suspended.
    NotSuspended,
    /// The bytes are not a snapshot this kernel can load (the message says
    /// why).
    BadSnapshot,
    /// No executable or file set has this name.
    NoImage,
    /// Memory cannot hold what the request needs.
    NoMemory,
    /// The kernel could not do it (the message says why).
    Failed,
    /// The requester has no authority over the cell, or a scoped request
    /// reaches outside its subtree or widens (version 4).
    Denied,
    /// A tree request the kernel cannot apply: a bad label, rule, limit or
    /// placement (version 4; the message says why).
    Invalid,
}

impl Error {
    fn code(self) -> u8 {
        match self {
            Error::Malformed => 1,
            Error::Version => 2,
            Error::TooLarge => 3,
            Error::UnknownKind => 4,
            Error::NoCell => 5,
            Error::NotRunning => 6,
            Error::NotSuspended => 7,
            Error::NoImage => 8,
            Error::NoMemory => 9,
            Error::Failed => 10,
            Error::BadSnapshot => 11,
            Error::Denied => 12,
            Error::Invalid => 13,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Error::Malformed,
            2 => Error::Version,
            3 => Error::TooLarge,
            4 => Error::UnknownKind,
            5 => Error::NoCell,
            6 => Error::NotRunning,
            7 => Error::NotSuspended,
            8 => Error::NoImage,
            9 => Error::NoMemory,
            10 => Error::Failed,
            11 => Error::BadSnapshot,
            12 => Error::Denied,
            13 => Error::Invalid,
            _ => return None,
        })
    }

    /// A short description.
    pub fn as_str(self) -> &'static str {
        match self {
            Error::Malformed => "malformed request",
            Error::Version => "unsupported protocol version",
            Error::TooLarge => "request too large",
            Error::UnknownKind => "unknown request",
            Error::NoCell => "no such cell",
            Error::NotRunning => "the cell is not running",
            Error::NotSuspended => "the cell is not suspended",
            Error::NoImage => "no such executable or file set",
            Error::NoMemory => "not enough memory",
            Error::Failed => "failed",
            Error::BadSnapshot => "not a snapshot this kernel can load",
            Error::Denied => "not allowed",
            Error::Invalid => "invalid tree request",
        }
    }
}

/// One frame: its protocol version, kind, request identifier and payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub version: u8,
    pub kind: u8,
    pub id: u32,
    pub payload: Vec<u8>,
}

/// The bytes of a frame of the current version.
pub fn encode_frame(kind: u8, id: u32, payload: &[u8]) -> Vec<u8> {
    encode_frame_as(VERSION, kind, id, payload)
}

/// The bytes of a frame of `version` (a relay keeps the version it read).
pub fn encode_frame_as(version: u8, kind: u8, id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&MAGIC);
    out.push(version);
    out.push(kind);
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// What a [`Decoder`] found in the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// A byte of text between frames.
    Text(u8),
    /// A whole frame.
    Frame(Frame),
    /// A frame that cannot be read, with its identifier when its header
    /// had one.
    Bad { id: u32, error: Error },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    Text,
    Header,
    Payload {
        version: u8,
        kind: u8,
        id: u32,
        len: usize,
    },
    Skip {
        left: usize,
    },
}

/// Splits a byte stream into text and frames, one byte at a time. The
/// kernel's takes frames of [`VERSION`] only; the host's
/// ([`Decoder::any_version`]) takes every version, so that it reads the
/// hello and refusals of any kernel, and leaves the rest to
/// [`Message::decode`].
#[derive(Debug)]
pub struct Decoder {
    any_version: bool,
    state: Phase,
    header: [u8; HEADER_LEN],
    have: usize,
    payload: Vec<u8>,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    pub const fn new() -> Self {
        Self {
            any_version: false,
            state: Phase::Text,
            header: [0; HEADER_LEN],
            have: 0,
            payload: Vec::new(),
        }
    }

    /// A decoder that takes frames of every version (the host's).
    pub const fn any_version() -> Self {
        Self {
            any_version: true,
            state: Phase::Text,
            header: [0; HEADER_LEN],
            have: 0,
            payload: Vec::new(),
        }
    }

    /// Whether the decoder is inside a frame (more bytes are due).
    pub fn in_frame(&self) -> bool {
        self.state != Phase::Text
    }

    /// Takes the next byte of the stream.
    pub fn push(&mut self, byte: u8) -> Option<Item> {
        match self.state {
            Phase::Text if byte == MAGIC[0] => {
                self.header[0] = byte;
                self.have = 1;
                self.state = Phase::Header;
                None
            }
            Phase::Text => Some(Item::Text(byte)),
            Phase::Header => {
                if self.have == 1 && byte != MAGIC[1] {
                    // A stray 0x01: what follows it is text again.
                    self.state = Phase::Text;
                    return self.push(byte);
                }
                self.header[self.have] = byte;
                self.have += 1;
                if self.have < HEADER_LEN {
                    return None;
                }
                let h = &self.header;
                let id = u32::from_le_bytes([h[4], h[5], h[6], h[7]]);
                let len = u32::from_le_bytes([h[8], h[9], h[10], h[11]]) as usize;
                let (version, kind) = (h[2], h[3]);
                self.state = Phase::Text;
                if len > MAX_PAYLOAD {
                    // Its length cannot be trusted: look for the next frame.
                    return Some(Item::Bad {
                        id,
                        error: Error::TooLarge,
                    });
                }
                let error = if version != VERSION && !self.any_version {
                    Error::Version
                } else {
                    self.payload = Vec::new();
                    if self.payload.try_reserve_exact(len).is_ok() {
                        if len == 0 {
                            return Some(self.frame(version, kind, id));
                        }
                        self.state = Phase::Payload {
                            version,
                            kind,
                            id,
                            len,
                        };
                        return None;
                    }
                    Error::TooLarge
                };
                if len > 0 {
                    self.state = Phase::Skip { left: len };
                }
                Some(Item::Bad { id, error })
            }
            Phase::Payload {
                version,
                kind,
                id,
                len,
            } => {
                self.payload.push(byte);
                if self.payload.len() < len {
                    return None;
                }
                self.state = Phase::Text;
                Some(self.frame(version, kind, id))
            }
            Phase::Skip { left } => {
                self.state = if left > 1 {
                    Phase::Skip { left: left - 1 }
                } else {
                    Phase::Text
                };
                None
            }
        }
    }

    fn frame(&mut self, version: u8, kind: u8, id: u32) -> Item {
        Item::Frame(Frame {
            version,
            kind,
            id,
            payload: core::mem::take(&mut self.payload),
        })
    }
}

/// Appends little-endian fields to a payload.
#[derive(Debug, Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn host(&mut self, h: &Host) -> &mut Self {
        self.u32(u32::from_be_bytes(h.ip));
        self.0.extend_from_slice(&h.port.to_le_bytes());
        self.u64(h.token)
    }
    fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.u32(v.len() as u32);
        self.0.extend_from_slice(v);
        self
    }
    fn list<T>(&mut self, items: &[T], mut each: impl FnMut(&mut Self, &T)) -> &mut Self {
        self.u32(items.len() as u32);
        for item in items {
            each(self, item);
        }
        self
    }
    fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn limits(&mut self, l: &Limits) -> &mut Self {
        self.u64(l.memory).u64(l.cpu_ns).u32(l.sockets).u32(l.cells)
    }
    fn egress(&mut self, e: &Egress) -> &mut Self {
        match e {
            Egress::None => self.u8(0),
            Egress::Rules(rules) => self.u8(1).rules(rules),
            Egress::Full => self.u8(2),
        }
    }
    fn rules(&mut self, rules: &[Rule]) -> &mut Self {
        self.list(rules, |w, r| {
            w.u8(r.proto as u8);
            w.0.extend_from_slice(&r.addr);
            w.u8(r.prefix).u16(r.lo).u16(r.hi);
        })
    }
    fn ceiling(&mut self, c: &Option<Egress>) -> &mut Self {
        match c {
            None => self.u8(0),
            Some(e) => self.u8(1).egress(e),
        }
    }
    /// A placement: one byte 0 for the default (most cells, so that a
    /// `run` costs two bytes more than in version 3, where bytes are slow:
    /// the serial line), else 1 and its fields.
    fn placement(&mut self, p: &Placement) -> &mut Self {
        if p.is_root() {
            return self.u8(0);
        }
        self.u8(1).u32(p.parent).bytes(&p.label).u8(0); // mode: reserved
        self.limits(&p.limits)
            .egress(&p.egress)
            .ceiling(&p.ceiling)
            .u8(p.on_parent_end as u8)
            .u64(p.cpus)
            .u16(p.cpu_weight)
            .u32(p.cpu_max)
    }
}

/// Reads the fields of a payload, every read checked.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if self.0.len() < n {
            return Err(Error::Malformed);
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }
    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, Error> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64, Error> {
        let mut b = [0; 8];
        b.copy_from_slice(self.take(8)?);
        Ok(u64::from_le_bytes(b))
    }
    fn bytes(&mut self) -> Result<Vec<u8>, Error> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    fn host(&mut self) -> Result<Host, Error> {
        let ip = self.u32()?.to_be_bytes();
        let b = self.take(2)?;
        Ok(Host {
            ip,
            port: u16::from_le_bytes([b[0], b[1]]),
            token: self.u64()?,
        })
    }
    fn u16(&mut self) -> Result<u16, Error> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn limits(&mut self) -> Result<Limits, Error> {
        Ok(Limits {
            memory: self.u64()?,
            cpu_ns: self.u64()?,
            sockets: self.u32()?,
            cells: self.u32()?,
        })
    }
    fn egress(&mut self) -> Result<Egress, Error> {
        Ok(match self.u8()? {
            0 => Egress::None,
            1 => Egress::Rules(self.rules()?),
            2 => Egress::Full,
            _ => return Err(Error::Malformed),
        })
    }
    fn rules(&mut self) -> Result<Vec<Rule>, Error> {
        self.list(|r| {
            let proto = match r.u8()? {
                0 => Proto::Tcp,
                1 => Proto::Udp,
                2 => Proto::Any,
                _ => return Err(Error::Malformed),
            };
            let mut addr = [0; 4];
            addr.copy_from_slice(r.take(4)?);
            Ok(Rule {
                proto,
                addr,
                prefix: r.u8()?,
                lo: r.u16()?,
                hi: r.u16()?,
            })
        })
    }
    fn ceiling(&mut self) -> Result<Option<Egress>, Error> {
        Ok(if self.flag()? {
            Some(self.egress()?)
        } else {
            None
        })
    }
    fn placement(&mut self) -> Result<Placement, Error> {
        if !self.flag()? {
            return Ok(Placement::root());
        }
        let parent = self.u32()?;
        let label = self.bytes()?;
        if self.u8()? != 0 {
            return Err(Error::Malformed); // only `reserved` exists
        }
        Ok(Placement {
            parent,
            label,
            limits: self.limits()?,
            egress: self.egress()?,
            ceiling: self.ceiling()?,
            on_parent_end: match self.u8()? {
                0 => OnParentEnd::Cascade,
                1 => OnParentEnd::Promote,
                _ => return Err(Error::Malformed),
            },
            cpus: self.u64()?,
            cpu_weight: self.u16()?,
            cpu_max: self.u32()?,
        })
    }
    fn flag(&mut self) -> Result<bool, Error> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::Malformed),
        }
    }
    /// A list: its count is bounded by the bytes left (every item takes at
    /// least one), so a forged count allocates nothing.
    fn list<T>(
        &mut self,
        mut each: impl FnMut(&mut Self) -> Result<T, Error>,
    ) -> Result<Vec<T>, Error> {
        let n = self.u32()? as usize;
        if n > self.0.len() {
            return Err(Error::Malformed);
        }
        let mut out = Vec::new();
        for _ in 0..n {
            out.push(each(self)?);
        }
        Ok(out)
    }
    fn end(self) -> Result<(), Error> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(Error::Malformed)
        }
    }
}

/// What a cell may use (version 4): memory in bytes, CPU time in
/// nanoseconds, sockets, and descendants; `u64::MAX` or `u32::MAX` is no
/// limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub memory: u64,
    pub cpu_ns: u64,
    pub sockets: u32,
    pub cells: u32,
}

impl Limits {
    /// No limit on anything: for a root, what the kernel's quotas allow.
    pub const NONE: Self = Self {
        memory: u64::MAX,
        cpu_ns: u64::MAX,
        sockets: u32::MAX,
        cells: u32::MAX,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::NONE
    }
}

/// The protocol an egress rule allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proto {
    Tcp = 0,
    Udp = 1,
    Any = 2,
}

/// One egress rule (design D7 of `add-cell-hierarchy`): a protocol, an
/// IPv4 network and an inclusive range of ports. Decoding checks only its
/// shape; the kernel refuses, with the reason, a prefix over 32 or an
/// empty range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    pub proto: Proto,
    pub addr: [u8; 4],
    pub prefix: u8,
    pub lo: u16,
    pub hi: u16,
}

/// An egress policy: nothing outside the cell, some rules, or everything
/// the deny floor leaves. `model` is a name `rhizo` gives rules: the model
/// endpoint as the guest reaches it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Egress {
    #[default]
    None,
    Rules(Vec<Rule>),
    Full,
}

/// What happens to a cell when its parent ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnParentEnd {
    #[default]
    Cascade = 0,
    Promote = 1,
}

/// Where a cell the host makes goes in the tree, and what it may do
/// (version 4, design D14 of `add-cell-hierarchy`), for `run`, `load` and
/// the fork of a template. The default is a root with egress `none`,
/// limited by the kernel's quotas only, as version 3 made every cell.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Placement {
    /// Its parent (0: a root, the host's).
    pub parent: u32,
    /// Its label, `[A-Za-z0-9._-]`, empty for none.
    pub label: Vec<u8>,
    /// Its limits, reserved in full from its parent's budget.
    pub limits: Limits,
    pub egress: Egress,
    /// Its exposure ceiling; `None` takes the egress policy.
    pub ceiling: Option<Egress>,
    pub on_parent_end: OnParentEnd,
    /// An exclusive set of CPUs, one bit each (roots only; 0: none).
    pub cpus: u64,
    /// CPU weight among its siblings (0: the default, 100).
    pub cpu_weight: u16,
    /// CPU cap of its subtree in thousandths of a CPU (`u32::MAX`: none).
    pub cpu_max: u32,
}

impl Placement {
    /// The default placement: a root as version 3 made them.
    pub fn root() -> Self {
        Self {
            cpu_max: u32::MAX,
            ..Self::default()
        }
    }

    /// Whether version 3 can say it (it is [`Placement::root`]).
    pub fn is_root(&self) -> bool {
        *self == Self::root()
    }
}

/// What `run` starts (spec `control-plane`, commands).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// The executable: the name of one the kernel holds.
    pub image: Vec<u8>,
    /// The files the cell starts with: the name of a file set the kernel
    /// holds, or empty for none.
    pub files_from: Vec<u8>,
    /// Its arguments, `argv[0]` first.
    pub argv: Vec<Vec<u8>>,
    /// Its environment (`NAME=VALUE`).
    pub envp: Vec<Vec<u8>>,
    /// Files written into its files before it starts: path and contents.
    pub files: Vec<(Vec<u8>, Vec<u8>)>,
    /// Whether its standard input comes from the host ([`Request::Input`]);
    /// without it, reading descriptor 0 gives end of file.
    pub input: bool,
    /// Where it goes in the tree (version 4).
    pub placement: Placement,
    /// Whether its output streams to this request, right after the reply,
    /// as an `output` request would (version 4): no second round trip.
    pub watch: bool,
}

impl Default for Run {
    fn default() -> Self {
        Self {
            image: Vec::new(),
            files_from: Vec::new(),
            argv: Vec::new(),
            envp: Vec::new(),
            files: Vec::new(),
            input: false,
            placement: Placement::root(),
            watch: false,
        }
    }
}

/// A request, host to kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Start a cell; replies [`Body::Cell`].
    Run(Run),
    /// Every cell the kernel knows; replies [`Body::List`].
    List,
    /// Suspend a running cell into a snapshot; replies [`Body::Bytes`],
    /// the snapshot's size.
    Suspend(u32),
    /// Resume a suspended cell, same identifier.
    Resume(u32),
    /// Fork a running cell into `n` new ones, its siblings, or make `n`
    /// cells of a template, placed by `placement` (version 4: a running
    /// cell's copies take only the default); replies [`Body::Cells`]. With
    /// `watch`, their output streams to this request (version 4).
    Fork {
        cell: u32,
        n: u32,
        placement: Placement,
        watch: bool,
    },
    /// End a cell, or forget one that ended.
    Kill(u32),
    /// End a cell and its whole subtree, whatever their policy for when a
    /// parent ends (version 4).
    KillTree(u32),
    /// Suspend every running cell of a subtree (version 4); replies
    /// [`Body::Cells`], those suspended.
    SuspendTree(u32),
    /// Resume every suspended cell of a subtree (version 4); replies
    /// [`Body::Cells`], those resumed.
    ResumeTree(u32),
    /// Move a cell under `to`, or make it a root (`to` 0) (version 4).
    Reparent { cell: u32, to: u32 },
    /// Set a cell's egress policy and ceiling: narrowing for a non-root,
    /// any for a root (version 4).
    Policy {
        cell: u32,
        egress: Egress,
        ceiling: Option<Egress>,
    },
    /// Set a cell's limits, within its parent's free budget (version 4).
    Limits { cell: u32, limits: Limits },
    /// `request` from a client limited to `root`'s subtree (version 4,
    /// `rhizo scope`): the kernel refuses any target outside it, a move
    /// out of it, and widening. Never nested.
    Scoped {
        root: u32,
        request: alloc::boxed::Box<Request>,
    },
    /// Stream the cell's output: what it printed so far, then as it prints,
    /// as [`Message::Output`], then [`Message::Ended`] when it ends.
    Output(u32),
    /// Create or replace a file in a running cell's files.
    WriteFile {
        cell: u32,
        path: Vec<u8>,
        data: Vec<u8>,
    },
    /// Memory and cells; replies [`Body::Stats`].
    Stats,
    /// Power the machine off after replying.
    Shutdown,
    /// Write bytes into a running cell's standard input (a cell started
    /// with [`Run::input`]), whole or not at all; at most [`MAX_INPUT`].
    Input { cell: u32, data: Vec<u8> },
    /// Close the host's end of a running cell's standard input: once the
    /// cell has read what is left, it reads end of file.
    CloseInput(u32),
    /// Freeze a running cell into a template: it never runs again, and
    /// [`Request::Fork`] makes cells from it.
    Freeze(u32),
    /// Send the snapshot of a suspended cell or a template, as bytes that
    /// stand alone, over TCP to `port` at `host` (the host as the guest
    /// reaches it, through the network card); replies [`Body::Bytes`],
    /// their total, once the host has them all.
    Save { cell: u32, host: Host },
    /// Publish a template (version 4, design D21 of `add-cell-hierarchy`):
    /// its copies placed in any domain share its frames. Refused for a
    /// template whose cell reached the network card unless `attest`: the
    /// host's word that it holds no secret, which the kernel audits.
    Publish { cell: u32, attest: bool },
    /// End a template's publication (version 4): later copies placed in
    /// another domain are full copies.
    Withdraw(u32),
    /// Fetch a snapshot of `size` bytes over TCP from `port` at `host`, and
    /// make it a suspended cell, or a template; replies [`Body::Cell`].
    Load {
        size: u64,
        template: bool,
        host: Host,
        /// Where the cell goes (version 4): a template at once, a
        /// suspended cell when it is resumed.
        placement: Placement,
    },
    /// The deny floor no egress policy reaches (version 4, design D7 of
    /// `add-cell-hierarchy`), in place of the one before: `deny`, beyond
    /// the host's own addresses as the guest sees them (always in it), and
    /// the exact endpoints `carved` out of it (the model's). Sent by
    /// `rhizo up` after the hello; never scoped.
    Floor { deny: Vec<Rule>, carved: Vec<Rule> },
}

/// Where the guest reaches the host's end of a transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Host {
    pub ip: [u8; 4],
    pub port: u16,
    /// A random number for this transfer only: the kernel sends it first
    /// on the connection, and the host takes no other (design D2 of
    /// `snapshots-on-disk`).
    pub token: u64,
}

impl Request {
    /// The `n` copies of `cell`, placed as the defaults say.
    pub fn fork(cell: u32, n: u32) -> Self {
        Request::Fork {
            cell,
            n,
            placement: Placement::root(),
            watch: false,
        }
    }

    /// The frame of this request, in the current version.
    pub fn encode(&self, id: u32) -> Vec<u8> {
        // Every request has a frame in the current version.
        self.encode_as(VERSION, id).unwrap_or_default()
    }

    /// The frame of this request in `version` ([`VERSION`] or
    /// [`PREVIOUS`]); [`Error::Version`] if that version cannot say it
    /// (a tree request, a placement, a watch).
    pub fn encode_as(&self, version: u8, id: u32) -> Result<Vec<u8>, Error> {
        let v4 = match version {
            VERSION => true,
            PREVIOUS => false,
            _ => return Err(Error::Version),
        };
        let mut w = Writer::default();
        let kind = match self {
            Request::Run(run) => {
                w.bytes(&run.image).bytes(&run.files_from);
                w.list(&run.argv, |w, a| {
                    w.bytes(a);
                });
                w.list(&run.envp, |w, e| {
                    w.bytes(e);
                });
                w.list(&run.files, |w, (path, data)| {
                    w.bytes(path).bytes(data);
                });
                w.u8(run.input as u8);
                if v4 {
                    w.placement(&run.placement).u8(run.watch as u8);
                } else if !run.placement.is_root() || run.watch {
                    return Err(Error::Version);
                }
                kind::RUN
            }
            Request::List => kind::LIST,
            Request::Suspend(cell) | Request::SuspendTree(cell) => {
                w.u32(*cell);
                tree_flag(&mut w, v4, matches!(self, Request::SuspendTree(_)))?;
                kind::SUSPEND
            }
            Request::Resume(cell) | Request::ResumeTree(cell) => {
                w.u32(*cell);
                tree_flag(&mut w, v4, matches!(self, Request::ResumeTree(_)))?;
                kind::RESUME
            }
            Request::Fork {
                cell,
                n,
                placement,
                watch,
            } => {
                w.u32(*cell).u32(*n);
                if v4 {
                    w.placement(placement).u8(*watch as u8);
                } else if !placement.is_root() || *watch {
                    return Err(Error::Version);
                }
                kind::FORK
            }
            Request::Kill(cell) | Request::KillTree(cell) => {
                w.u32(*cell);
                tree_flag(&mut w, v4, matches!(self, Request::KillTree(_)))?;
                kind::KILL
            }
            Request::Output(cell) => {
                w.u32(*cell);
                kind::OUTPUT
            }
            Request::WriteFile { cell, path, data } => {
                w.u32(*cell).bytes(path).bytes(data);
                kind::WRITE_FILE
            }
            Request::Stats => kind::STATS,
            Request::Shutdown => kind::SHUTDOWN,
            Request::Input { cell, data } => {
                w.u32(*cell).bytes(data);
                kind::INPUT
            }
            Request::CloseInput(cell) => {
                w.u32(*cell);
                kind::CLOSE_INPUT
            }
            Request::Freeze(cell) => {
                w.u32(*cell);
                kind::FREEZE
            }
            Request::Save { cell, host } => {
                w.u32(*cell).host(host);
                kind::SAVE
            }
            Request::Load {
                size,
                template,
                host,
                placement,
            } => {
                w.u64(*size).u8(*template as u8).host(host);
                if v4 {
                    w.placement(placement);
                } else if !placement.is_root() {
                    return Err(Error::Version);
                }
                kind::LOAD
            }
            _ if !v4 => return Err(Error::Version),
            Request::Reparent { cell, to } => {
                w.u32(*cell).u32(*to);
                kind::REPARENT
            }
            Request::Policy {
                cell,
                egress,
                ceiling,
            } => {
                w.u32(*cell).egress(egress).ceiling(ceiling);
                kind::POLICY
            }
            Request::Limits { cell, limits } => {
                w.u32(*cell).limits(limits);
                kind::LIMITS
            }
            Request::Floor { deny, carved } => {
                w.rules(deny).rules(carved);
                kind::FLOOR
            }
            Request::Publish { cell, attest } => {
                w.u32(*cell).u8(*attest as u8);
                kind::PUBLISH
            }
            Request::Withdraw(cell) => {
                w.u32(*cell);
                kind::WITHDRAW
            }
            Request::Scoped { root, request } => {
                // A nested one is written, and refused when read.
                let inner = request.encode_as(VERSION, 0)?;
                w.u32(*root).u8(inner[3]).bytes(&inner[HEADER_LEN..]);
                kind::SCOPED
            }
        };
        Ok(encode_frame_as(version, kind, id, &w.0))
    }

    /// The request a frame carries, in its version ([`VERSION`] or
    /// [`PREVIOUS`]).
    pub fn decode(frame: &Frame) -> Result<Self, Error> {
        let v4 = match frame.version {
            VERSION => true,
            PREVIOUS => false,
            _ => return Err(Error::Version),
        };
        let mut r = Reader(&frame.payload);
        let request = Self::read(frame.kind, &mut r, v4, true)?;
        r.end()?;
        Ok(request)
    }

    /// The request of `kind` in `r`; `outer`: it may be a scoped one.
    fn read(kind: u8, r: &mut Reader<'_>, v4: bool, outer: bool) -> Result<Self, Error> {
        let tree = |r: &mut Reader<'_>| if v4 { r.flag() } else { Ok(false) };
        let request = match kind {
            kind::RUN => Request::Run(Run {
                image: r.bytes()?,
                files_from: r.bytes()?,
                argv: r.list(Reader::bytes)?,
                envp: r.list(Reader::bytes)?,
                files: r.list(|r| Ok((r.bytes()?, r.bytes()?)))?,
                input: r.flag()?,
                placement: if v4 {
                    r.placement()?
                } else {
                    Placement::root()
                },
                watch: v4 && r.flag()?,
            }),
            kind::LIST => Request::List,
            kind::SUSPEND => {
                let cell = r.u32()?;
                if tree(r)? {
                    Request::SuspendTree(cell)
                } else {
                    Request::Suspend(cell)
                }
            }
            kind::RESUME => {
                let cell = r.u32()?;
                if tree(r)? {
                    Request::ResumeTree(cell)
                } else {
                    Request::Resume(cell)
                }
            }
            kind::FORK => Request::Fork {
                cell: r.u32()?,
                n: r.u32()?,
                placement: if v4 {
                    r.placement()?
                } else {
                    Placement::root()
                },
                watch: v4 && r.flag()?,
            },
            kind::KILL => {
                let cell = r.u32()?;
                if tree(r)? {
                    Request::KillTree(cell)
                } else {
                    Request::Kill(cell)
                }
            }
            kind::OUTPUT => Request::Output(r.u32()?),
            kind::WRITE_FILE => Request::WriteFile {
                cell: r.u32()?,
                path: r.bytes()?,
                data: r.bytes()?,
            },
            kind::STATS => Request::Stats,
            kind::SHUTDOWN => Request::Shutdown,
            kind::INPUT => {
                let cell = r.u32()?;
                let data = r.bytes()?;
                if data.len() > MAX_INPUT {
                    return Err(Error::TooLarge);
                }
                Request::Input { cell, data }
            }
            kind::CLOSE_INPUT => Request::CloseInput(r.u32()?),
            kind::FREEZE => Request::Freeze(r.u32()?),
            kind::SAVE => Request::Save {
                cell: r.u32()?,
                host: r.host()?,
            },
            kind::LOAD => Request::Load {
                size: r.u64()?,
                template: r.flag()?,
                host: r.host()?,
                placement: if v4 {
                    r.placement()?
                } else {
                    Placement::root()
                },
            },
            _ if !v4 => return Err(Error::UnknownKind),
            kind::REPARENT => Request::Reparent {
                cell: r.u32()?,
                to: r.u32()?,
            },
            kind::POLICY => Request::Policy {
                cell: r.u32()?,
                egress: r.egress()?,
                ceiling: r.ceiling()?,
            },
            kind::LIMITS => Request::Limits {
                cell: r.u32()?,
                limits: r.limits()?,
            },
            kind::PUBLISH => Request::Publish {
                cell: r.u32()?,
                attest: r.flag()?,
            },
            kind::WITHDRAW => Request::Withdraw(r.u32()?),
            kind::SCOPED if outer => {
                let root = r.u32()?;
                let kind = r.u8()?;
                let payload = r.bytes()?;
                let mut inner = Reader(&payload);
                let request = Self::read(kind, &mut inner, true, false)?;
                inner.end()?;
                Request::Scoped {
                    root,
                    request: alloc::boxed::Box::new(request),
                }
            }
            kind::SCOPED => return Err(Error::Malformed), // nested
            kind::FLOOR => Request::Floor {
                deny: r.rules()?,
                carved: r.rules()?,
            },
            _ => return Err(Error::UnknownKind),
        };
        Ok(request)
    }
}

/// The `tree` flag of `kill`, `suspend` and `resume`: in version 4 only.
fn tree_flag(w: &mut Writer, v4: bool, tree: bool) -> Result<(), Error> {
    if v4 {
        w.u8(tree as u8);
    } else if tree {
        return Err(Error::Version);
    }
    Ok(())
}

/// Where a cell is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Running,
    /// In a snapshot of this many bytes.
    Suspended {
        bytes: u64,
    },
    /// It exited with this status.
    Exited(i32),
    /// It raised an exception and was ended.
    Fault,
    /// It was killed.
    Killed,
    /// It is a template.
    Template,
}

impl State {
    fn write(self, w: &mut Writer) {
        match self {
            State::Running => {
                w.u8(0);
            }
            State::Suspended { bytes } => {
                w.u8(1).u64(bytes);
            }
            State::Exited(code) => {
                w.u8(2).u32(code as u32);
            }
            State::Fault => {
                w.u8(3);
            }
            State::Killed => {
                w.u8(4);
            }
            State::Template => {
                w.u8(5);
            }
        }
    }

    fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(match r.u8()? {
            0 => State::Running,
            1 => State::Suspended { bytes: r.u64()? },
            2 => State::Exited(r.u32()? as i32),
            3 => State::Fault,
            4 => State::Killed,
            5 => State::Template,
            _ => return Err(Error::Malformed),
        })
    }
}

/// One cell in a [`Body::List`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellInfo {
    pub id: u32,
    /// The executable it was started from.
    pub image: Vec<u8>,
    pub state: State,
    /// Its place in the tree (version 4); `None` for a cell that ended, a
    /// loaded one not yet resumed, or from a version-3 kernel.
    pub tree: Option<TreeInfo>,
}

/// A cell's place in the tree and its policies, in a [`Body::List`]
/// (version 4).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TreeInfo {
    /// Its parent (0: it is a root).
    pub parent: u32,
    /// The root of its domain (itself for a root; 0 if the host knows no
    /// such cell).
    pub root: u32,
    pub label: Vec<u8>,
    pub limits: Limits,
    pub egress: Egress,
}

/// The kernel's memory and cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    pub running: u32,
    pub suspended: u32,
    pub ended: u32,
    pub templates: u32,
    /// Bytes held by snapshots of suspended cells.
    pub snapshot_bytes: u64,
    pub memory_total: u64,
    pub memory_free: u64,
}

/// What a successful request returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    None,
    Cell(u32),
    Cells(Vec<u32>),
    Bytes(u64),
    List(Vec<CellInfo>),
    Stats(Stats),
}

/// A message, kernel to host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// The kernel serves requests (identifier 0).
    Hello { version: u8 },
    /// A request succeeded in `ns` nanoseconds.
    Done { ns: u64, body: Body },
    /// A request failed after `ns` nanoseconds.
    Failed {
        ns: u64,
        error: Error,
        message: Vec<u8>,
    },
    /// Output of a cell, for an `output` request.
    Output { cell: u32, data: Vec<u8> },
    /// The cell of an `output` request ended.
    Ended { cell: u32, state: State },
    /// An audit record (version 4, identifier 0), sent unasked.
    Audit(audit::Record),
}

impl Message {
    /// The frame of this message, in the current version.
    pub fn encode(&self, id: u32) -> Vec<u8> {
        self.encode_as(VERSION, id)
    }

    /// The frame of this message in `version` ([`PREVIOUS`] leaves out
    /// what version 4 added; any other is written as the current one).
    pub fn encode_as(&self, version: u8, id: u32) -> Vec<u8> {
        let v4 = version != PREVIOUS;
        let mut w = Writer::default();
        let kind = match self {
            Message::Hello { version } => {
                w.u8(*version);
                kind::HELLO
            }
            Message::Done { ns, body } => {
                w.u64(*ns);
                match body {
                    Body::None => {
                        w.u8(0);
                    }
                    Body::Cell(id) => {
                        w.u8(1).u32(*id);
                    }
                    Body::Cells(ids) => {
                        w.u8(2).list(ids, |w, id| {
                            w.u32(*id);
                        });
                    }
                    Body::Bytes(n) => {
                        w.u8(3).u64(*n);
                    }
                    Body::List(cells) => {
                        w.u8(4).list(cells, |w, c| {
                            w.u32(c.id).bytes(&c.image);
                            c.state.write(w);
                            if v4 {
                                match &c.tree {
                                    None => {
                                        w.u8(0);
                                    }
                                    Some(t) => {
                                        w.u8(1).u32(t.parent).u32(t.root).bytes(&t.label);
                                        w.limits(&t.limits).egress(&t.egress);
                                    }
                                }
                            }
                        });
                    }
                    Body::Stats(s) => {
                        w.u8(5)
                            .u32(s.running)
                            .u32(s.suspended)
                            .u32(s.ended)
                            .u32(s.templates)
                            .u64(s.snapshot_bytes)
                            .u64(s.memory_total)
                            .u64(s.memory_free);
                    }
                }
                kind::DONE
            }
            Message::Failed { ns, error, message } => {
                w.u64(*ns).u8(error.code()).bytes(message);
                kind::FAILED
            }
            Message::Output { cell, data } => {
                w.u32(*cell).bytes(data);
                kind::OUTPUT_DATA
            }
            Message::Ended { cell, state } => {
                w.u32(*cell);
                state.write(&mut w);
                kind::ENDED
            }
            Message::Audit(record) => {
                record.write(&mut w);
                kind::AUDIT
            }
        };
        encode_frame_as(version, kind, id, &w.0)
    }

    /// The message a frame carries, in its version: a hello or a refusal
    /// of any version (so that the host can name a kernel it does not
    /// speak), anything else in [`VERSION`] or [`PREVIOUS`].
    pub fn decode(frame: &Frame) -> Result<Self, Error> {
        let v4 = frame.version == VERSION;
        if !v4
            && frame.version != PREVIOUS
            && frame.kind != kind::HELLO
            && frame.kind != kind::FAILED
        {
            return Err(Error::Version);
        }
        let mut r = Reader(&frame.payload);
        let message = match frame.kind {
            kind::HELLO => Message::Hello { version: r.u8()? },
            kind::DONE => {
                let ns = r.u64()?;
                let body = match r.u8()? {
                    0 => Body::None,
                    1 => Body::Cell(r.u32()?),
                    2 => Body::Cells(r.list(Reader::u32)?),
                    3 => Body::Bytes(r.u64()?),
                    4 => Body::List(r.list(|r| {
                        Ok(CellInfo {
                            id: r.u32()?,
                            image: r.bytes()?,
                            state: State::read(r)?,
                            tree: if v4 && r.flag()? {
                                Some(TreeInfo {
                                    parent: r.u32()?,
                                    root: r.u32()?,
                                    label: r.bytes()?,
                                    limits: r.limits()?,
                                    egress: r.egress()?,
                                })
                            } else {
                                None
                            },
                        })
                    })?),
                    5 => Body::Stats(Stats {
                        running: r.u32()?,
                        suspended: r.u32()?,
                        ended: r.u32()?,
                        templates: r.u32()?,
                        snapshot_bytes: r.u64()?,
                        memory_total: r.u64()?,
                        memory_free: r.u64()?,
                    }),
                    _ => return Err(Error::Malformed),
                };
                Message::Done { ns, body }
            }
            kind::FAILED => Message::Failed {
                ns: r.u64()?,
                error: Error::from_code(r.u8()?).ok_or(Error::Malformed)?,
                message: r.bytes()?,
            },
            kind::OUTPUT_DATA => Message::Output {
                cell: r.u32()?,
                data: r.bytes()?,
            },
            kind::ENDED => Message::Ended {
                cell: r.u32()?,
                state: State::read(&mut r)?,
            },
            kind::AUDIT if v4 => Message::Audit(audit::Record::read(&mut r)?),
            _ => return Err(Error::UnknownKind),
        };
        r.end()?;
        Ok(message)
    }
}

#[cfg(test)]
mod tests;
