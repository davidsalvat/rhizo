//! The control protocol between the host and a control kernel (spec
//! `control-plane`, design D2 of `add-control-plane`): the host asks the
//! kernel to run, list, suspend, resume, fork and kill cells, and the kernel
//! answers each request with its outcome and the time it took.
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

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec::Vec;

/// The protocol's version, in every frame (2: standard input, design D2 of
/// `add-interactive-chat`).
pub const VERSION: u8 = 2;
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
    // Kernel to host.
    pub const HELLO: u8 = 0x80;
    pub const DONE: u8 = 0x81;
    pub const FAILED: u8 = 0x82;
    pub const OUTPUT_DATA: u8 = 0x83;
    pub const ENDED: u8 = 0x84;
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
    /// No executable or file set has this name.
    NoImage,
    /// Memory cannot hold what the request needs.
    NoMemory,
    /// The kernel could not do it (the message says why).
    Failed,
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
        }
    }
}

/// One frame: its kind, its request identifier and its payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: u8,
    pub id: u32,
    pub payload: Vec<u8>,
}

/// The bytes of a frame.
pub fn encode_frame(kind: u8, id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&MAGIC);
    out.push(VERSION);
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
    Payload { kind: u8, id: u32, len: usize },
    Skip { left: usize },
}

/// Splits a byte stream into text and frames, one byte at a time.
#[derive(Debug)]
pub struct Decoder {
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
                let error = if version != VERSION {
                    Error::Version
                } else {
                    self.payload = Vec::new();
                    if self.payload.try_reserve_exact(len).is_ok() {
                        if len == 0 {
                            return Some(self.frame(kind, id));
                        }
                        self.state = Phase::Payload { kind, id, len };
                        return None;
                    }
                    Error::TooLarge
                };
                if len > 0 {
                    self.state = Phase::Skip { left: len };
                }
                Some(Item::Bad { id, error })
            }
            Phase::Payload { kind, id, len } => {
                self.payload.push(byte);
                if self.payload.len() < len {
                    return None;
                }
                self.state = Phase::Text;
                Some(self.frame(kind, id))
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

    fn frame(&mut self, kind: u8, id: u32) -> Item {
        Item::Frame(Frame {
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

/// What `run` starts (spec `control-plane`, commands).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
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
    /// Fork a running cell into `n` new ones; replies [`Body::Cells`].
    Fork { cell: u32, n: u32 },
    /// End a cell, or forget one that ended.
    Kill(u32),
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
}

impl Request {
    /// The frame of this request.
    pub fn encode(&self, id: u32) -> Vec<u8> {
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
                kind::RUN
            }
            Request::List => kind::LIST,
            Request::Suspend(cell) => {
                w.u32(*cell);
                kind::SUSPEND
            }
            Request::Resume(cell) => {
                w.u32(*cell);
                kind::RESUME
            }
            Request::Fork { cell, n } => {
                w.u32(*cell).u32(*n);
                kind::FORK
            }
            Request::Kill(cell) => {
                w.u32(*cell);
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
        };
        encode_frame(kind, id, &w.0)
    }

    /// The request a frame carries.
    pub fn decode(frame: &Frame) -> Result<Self, Error> {
        let mut r = Reader(&frame.payload);
        let request = match frame.kind {
            kind::RUN => Request::Run(Run {
                image: r.bytes()?,
                files_from: r.bytes()?,
                argv: r.list(Reader::bytes)?,
                envp: r.list(Reader::bytes)?,
                files: r.list(|r| Ok((r.bytes()?, r.bytes()?)))?,
                input: match r.u8()? {
                    0 => false,
                    1 => true,
                    _ => return Err(Error::Malformed),
                },
            }),
            kind::LIST => Request::List,
            kind::SUSPEND => Request::Suspend(r.u32()?),
            kind::RESUME => Request::Resume(r.u32()?),
            kind::FORK => Request::Fork {
                cell: r.u32()?,
                n: r.u32()?,
            },
            kind::KILL => Request::Kill(r.u32()?),
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
            _ => return Err(Error::UnknownKind),
        };
        r.end()?;
        Ok(request)
    }
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
        }
    }

    fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(match r.u8()? {
            0 => State::Running,
            1 => State::Suspended { bytes: r.u64()? },
            2 => State::Exited(r.u32()? as i32),
            3 => State::Fault,
            4 => State::Killed,
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
}

/// The kernel's memory and cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    pub running: u32,
    pub suspended: u32,
    pub ended: u32,
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
}

impl Message {
    /// The frame of this message.
    pub fn encode(&self, id: u32) -> Vec<u8> {
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
                        });
                    }
                    Body::Stats(s) => {
                        w.u8(5)
                            .u32(s.running)
                            .u32(s.suspended)
                            .u32(s.ended)
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
        };
        encode_frame(kind, id, &w.0)
    }

    /// The message a frame carries.
    pub fn decode(frame: &Frame) -> Result<Self, Error> {
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
                        })
                    })?),
                    5 => Body::Stats(Stats {
                        running: r.u32()?,
                        suspended: r.u32()?,
                        ended: r.u32()?,
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
            _ => return Err(Error::UnknownKind),
        };
        r.end()?;
        Ok(message)
    }
}

#[cfg(test)]
mod tests;
