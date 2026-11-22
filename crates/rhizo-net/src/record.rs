//! What a snapshot keeps of a socket (design D4 of `add-sockets-and-poll`),
//! and its encoding: explicit little endian, every field checked on decode.
//!
//! Per record: `family: u8` (0 Inet, 1 Unix), `type: u8` (0 stream,
//! 1 datagram), `state: u8` (0 open, 1 listening, 2 reset), `backlog: u32`,
//! `parked: u8` (1: a listener or connection parked in the kernel, then
//! `boot: u64` and `key: u64`; a parked reset connection has no address),
//! the address (`tag: u8`: 0 none; 1 Inet, `ip: [u8; 4]`, `port: u16`;
//! 2 Unix, `name: u64`, `len: u16`, path; 3 unnamed Unix), then the options
//! (`flags: u8`: 1 reuse address, 2 reuse port, 4 keepalive, 8 no delay,
//! 16 broadcast, 32 linger; `linger: u32`, `rcvbuf: u32`, `sndbuf: u32`,
//! `rcv_timeout_ns: u64`, `snd_timeout_ns: u64`, `keep_idle: u32`,
//! `keep_intvl: u32`, `keep_cnt: u32`). A list is `count: u32` and records.

use super::*;

/// The state a socket comes back in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Saved {
    /// Not connected (bound if it has an address).
    Open,
    /// Listening with this backlog.
    Listening(u32),
    /// A connection, reset.
    Reset,
}

/// Where a listener or connection of a suspended cell waits in the kernel
/// (design D2 of `keep-tcp-across-suspend`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Park {
    /// Identity of the boot that parked it.
    pub boot: u64,
    pub key: u64,
}

/// A socket as a snapshot holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub family: Family,
    pub ty: Type,
    /// The state it comes back in when it is not parked in this boot.
    pub state: Saved,
    pub local: Option<Addr>,
    pub opts: Options,
    pub parked: Option<Park>,
}

/// Why encoded records are refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// The bytes end early or have bytes left over.
    Truncated,
    /// A field is out of range.
    Corrupt,
}

/// Appends `records` to `out`.
pub fn encode_records(records: &[Record], out: &mut Vec<u8>) {
    out.extend_from_slice(&(records.len() as u32).to_le_bytes());
    for r in records {
        out.push(match r.family {
            Family::Inet => 0,
            Family::Unix => 1,
        });
        out.push(match r.ty {
            Type::Stream => 0,
            Type::Dgram => 1,
        });
        let (state, backlog) = match r.state {
            Saved::Open => (0, 0),
            Saved::Listening(b) => (1, b),
            Saved::Reset => (2, 0),
        };
        out.push(state);
        out.extend_from_slice(&backlog.to_le_bytes());
        match r.parked {
            None => out.push(0),
            Some(p) => {
                out.push(1);
                out.extend_from_slice(&p.boot.to_le_bytes());
                out.extend_from_slice(&p.key.to_le_bytes());
            }
        }
        match &r.local {
            None => out.push(0),
            Some(Addr::Inet { ip, port }) => {
                out.push(1);
                out.extend_from_slice(ip);
                out.extend_from_slice(&port.to_le_bytes());
            }
            Some(Addr::Unix { name, path }) => {
                out.push(2);
                out.extend_from_slice(&name.to_le_bytes());
                out.extend_from_slice(&(path.len() as u16).to_le_bytes());
                out.extend_from_slice(path);
            }
            Some(Addr::UnixUnnamed) => out.push(3),
        }
        let o = &r.opts;
        let flags = o.reuse_addr as u8
            | (o.reuse_port as u8) << 1
            | (o.keepalive as u8) << 2
            | (o.no_delay as u8) << 3
            | (o.broadcast as u8) << 4
            | (o.linger.is_some() as u8) << 5;
        out.push(flags);
        for v in [o.linger.unwrap_or(0), o.rcvbuf, o.sndbuf] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in [o.rcv_timeout_ns, o.snd_timeout_ns] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in [o.keep_idle, o.keep_intvl, o.keep_cnt] {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> core::result::Result<&[u8], DecodeError> {
        if self.0.len() < n {
            return Err(DecodeError::Truncated);
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }
    fn u8(&mut self) -> core::result::Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> core::result::Result<u16, DecodeError> {
        Ok(u16::from_le_bytes([self.u8()?, self.u8()?]))
    }
    fn u32(&mut self) -> core::result::Result<u32, DecodeError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> core::result::Result<u64, DecodeError> {
        Ok(u64::from(self.u32()?) | u64::from(self.u32()?) << 32)
    }
}

/// Decodes a list written by [`encode_records`]; `bytes` must hold exactly
/// one.
pub fn decode_records(bytes: &[u8]) -> core::result::Result<Vec<Record>, DecodeError> {
    use DecodeError::Corrupt;
    // Most cells resumed have no socket: an empty list, at once.
    if bytes == [0; 4] {
        return Ok(Vec::new());
    }
    let mut r = Reader(bytes);
    let count = r.u32()? as usize;
    if count > bytes.len() {
        return Err(DecodeError::Truncated);
    }
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        let family = match r.u8()? {
            0 => Family::Inet,
            1 => Family::Unix,
            _ => return Err(Corrupt),
        };
        let ty = match r.u8()? {
            0 => Type::Stream,
            1 if family == Family::Inet => Type::Dgram,
            _ => return Err(Corrupt),
        };
        let (state, backlog) = (r.u8()?, r.u32()?);
        let state = match state {
            0 => Saved::Open,
            1 if ty == Type::Stream && backlog as usize <= BACKLOG_MAX => Saved::Listening(backlog),
            2 if ty == Type::Stream => Saved::Reset,
            _ => return Err(Corrupt),
        };
        let parked = match r.u8()? {
            0 => None,
            1 if matches!(state, Saved::Listening(_) | Saved::Reset) => Some(Park {
                boot: r.u64()?,
                key: r.u64()?,
            }),
            _ => return Err(Corrupt),
        };
        let local = match (r.u8()?, family) {
            (0, _) => None,
            (1, Family::Inet) => {
                let b = r.take(4)?;
                let ip = [b[0], b[1], b[2], b[3]];
                Some(Addr::Inet { ip, port: r.u16()? })
            }
            (2, Family::Unix) => {
                let name = r.u64()?;
                let len = r.u16()? as usize;
                if name == 0 || len == 0 || len > UNIX_PATH_MAX {
                    return Err(Corrupt);
                }
                Some(Addr::Unix {
                    name,
                    path: r.take(len)?.to_vec(),
                })
            }
            (3, Family::Unix) => Some(Addr::UnixUnnamed),
            _ => return Err(Corrupt),
        };
        if parked.is_some() && state == Saved::Reset && local.is_some() {
            return Err(Corrupt);
        }
        let flags = r.u8()?;
        if flags & !0x3f != 0 {
            return Err(Corrupt);
        }
        let linger = r.u32()?;
        let opts = Options {
            reuse_addr: flags & 1 != 0,
            reuse_port: flags & 2 != 0,
            keepalive: flags & 4 != 0,
            no_delay: flags & 8 != 0,
            broadcast: flags & 16 != 0,
            linger: (flags & 32 != 0).then_some(linger),
            rcvbuf: r.u32()?,
            sndbuf: r.u32()?,
            rcv_timeout_ns: r.u64()?,
            snd_timeout_ns: r.u64()?,
            keep_idle: r.u32()?,
            keep_intvl: r.u32()?,
            keep_cnt: r.u32()?,
        };
        records.push(Record {
            family,
            ty,
            state,
            local,
            opts,
            parked,
        });
    }
    if !r.0.is_empty() {
        return Err(DecodeError::Truncated);
    }
    Ok(records)
}
