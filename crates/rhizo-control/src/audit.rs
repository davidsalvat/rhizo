//! Audit records (design D18 of `add-cell-hierarchy`): what the kernel
//! refused (denials) and what the host administered, and the ring of
//! [`RING_BYTES`] that keeps records until they leave for the host.
//!
//! The ring never blocks and never grows: a record that does not fit is
//! counted as dropped, and the next one that fits carries the count. Every
//! record takes a sequence number, dropped ones too, so the host also sees
//! the gap. Repeated denials of one kind by one cell fold: the first of a
//! second goes in at once, the others are counted, and once that second is
//! over one more record carries their count, so that the counts of a
//! cell's records add up to its denials. Time is passed in, never read.

use super::{Error, Reader, Writer};
use alloc::collections::VecDeque;
use alloc::vec::Vec;

/// Bytes of records the ring holds that the host has not taken.
pub const RING_BYTES: usize = 64 << 10;
/// Longest detail a record keeps (longer ones are cut).
pub const DETAIL_MAX: usize = 200;
/// Cells whose denials fold at the same time; one more flushes the oldest.
const FOLDS: usize = 8;
const SECOND_NS: u64 = 1_000_000_000;
/// Bytes of a record before its detail.
const HEAD: usize = 41;

/// What a record is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A connection or datagram refused by an egress policy or the floor.
    Egress = 1,
    /// A scoped request reaching outside its subtree, or widening.
    Scope = 2,
    /// A tree request refused: no authority, or not applicable.
    Tree = 3,
    /// A loaded snapshot asked for more than it was given.
    Narrowed = 4,
    /// A template refused publication (it may hold a secret).
    Publish = 5,
    /// A connection to the control channel refused: it did not present
    /// the secret (change `authenticate-control-channel`).
    Auth = 6,
    /// Cells made by the host.
    Created = 16,
    /// A cell ended.
    Ended = 17,
    /// A cell moved in the tree.
    Moved = 18,
    /// A cell's egress policy and ceiling set.
    Policy = 19,
    /// A cell's limits set.
    Limits = 20,
    /// A template published on the host's attestation that it holds no
    /// secret.
    Attested = 21,
    /// A request from a client scoped to a subtree, let through.
    Scoped = 22,
}

impl Kind {
    pub const ALL: [Kind; 13] = [
        Kind::Egress,
        Kind::Scope,
        Kind::Tree,
        Kind::Narrowed,
        Kind::Publish,
        Kind::Auth,
        Kind::Created,
        Kind::Ended,
        Kind::Moved,
        Kind::Policy,
        Kind::Limits,
        Kind::Attested,
        Kind::Scoped,
    ];

    /// Whether it is a refusal (these fold).
    pub fn is_denial(self) -> bool {
        (self as u8) < 16
    }

    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|k| *k as u8 == code)
    }

    /// Its name, in the host's log and `rhizo audit tail --kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Egress => "egress-denied",
            Kind::Scope => "scope-denied",
            Kind::Tree => "tree-denied",
            Kind::Narrowed => "load-narrowed",
            Kind::Publish => "publish-refused",
            Kind::Auth => "auth-refused",
            Kind::Created => "created",
            Kind::Ended => "ended",
            Kind::Moved => "moved",
            Kind::Policy => "policy",
            Kind::Limits => "limits",
            Kind::Attested => "publish-attested",
            Kind::Scoped => "scoped",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == name)
    }
}

/// One audit record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// The kernel's sequence number, from 1, counting dropped records.
    pub seq: u64,
    /// The kernel's time, in nanoseconds since it booted.
    pub ns: u64,
    pub kind: Kind,
    /// The cell, and the root of its domain (0: none, or none the host
    /// can name).
    pub cell: u32,
    pub domain: u32,
    /// How many times it happened: 1, or for a folded record the denials
    /// of this kind by this cell in that second after the first.
    pub count: u32,
    /// Records dropped, the ring being full, since the record before.
    pub dropped: u64,
    /// What, for people (at most [`DETAIL_MAX`] bytes).
    pub detail: Vec<u8>,
}

/// The bytes of a record before its detail.
#[allow(clippy::too_many_arguments)]
fn head(
    seq: u64,
    ns: u64,
    kind: Kind,
    cell: u32,
    domain: u32,
    count: u32,
    dropped: u64,
    len: usize,
) -> [u8; HEAD] {
    let mut h = [0; HEAD];
    h[0..8].copy_from_slice(&seq.to_le_bytes());
    h[8..16].copy_from_slice(&ns.to_le_bytes());
    h[16] = kind as u8;
    h[17..21].copy_from_slice(&cell.to_le_bytes());
    h[21..25].copy_from_slice(&domain.to_le_bytes());
    h[25..29].copy_from_slice(&count.to_le_bytes());
    h[29..37].copy_from_slice(&dropped.to_le_bytes());
    h[37..41].copy_from_slice(&(len as u32).to_le_bytes());
    h
}

impl Record {
    pub(super) fn write(&self, w: &mut Writer) {
        let r = self;
        w.0.extend_from_slice(&head(
            r.seq,
            r.ns,
            r.kind,
            r.cell,
            r.domain,
            r.count,
            r.dropped,
            r.detail.len(),
        ));
        w.0.extend_from_slice(&r.detail);
    }

    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Record {
            seq: r.u64()?,
            ns: r.u64()?,
            kind: Kind::from_code(r.u8()?).ok_or(Error::Malformed)?,
            cell: r.u32()?,
            domain: r.u32()?,
            count: r.u32()?,
            dropped: r.u64()?,
            detail: r.bytes()?,
        })
    }
}

/// Denials of one kind by one cell in one second, after the first.
#[derive(Debug)]
struct Fold {
    kind: Kind,
    cell: u32,
    domain: u32,
    second: u64,
    count: u32,
    /// The first one's detail (the first destination).
    detail: Vec<u8>,
}

/// The records the host has not taken, at most [`RING_BYTES`] of them.
#[derive(Debug)]
pub struct Ring {
    /// Records as their payloads, each after its length (`u16`); room for
    /// [`RING_BYTES`] taken once, at the first record.
    bytes: VecDeque<u8>,
    /// The last sequence number given.
    seq: u64,
    /// Records dropped since the last that went in.
    dropped: u64,
    folds: Vec<Fold>,
}

impl Default for Ring {
    fn default() -> Self {
        Self::new()
    }
}

impl Ring {
    pub const fn new() -> Self {
        Self {
            bytes: VecDeque::new(),
            seq: 0,
            dropped: 0,
            folds: Vec::new(),
        }
    }

    /// Records `kind` about `cell` of domain `domain` at `now` (ns). It
    /// never waits: if the ring is full, the record is counted as dropped.
    pub fn record(&mut self, now: u64, kind: Kind, cell: u32, domain: u32, detail: &[u8]) {
        let detail = &detail[..detail.len().min(DETAIL_MAX)];
        self.expire(now);
        if kind.is_denial() {
            if let Some(f) = self
                .folds
                .iter_mut()
                .find(|f| f.kind == kind && f.cell == cell)
            {
                f.count = f.count.saturating_add(1);
                return;
            }
            if self.folds.len() == FOLDS {
                let oldest = self.folds.remove(0);
                self.flush(&oldest, now);
            }
            let mut first = Vec::new();
            if self.folds.try_reserve(FOLDS).is_ok() {
                if first.try_reserve_exact(detail.len()).is_ok() {
                    first.extend_from_slice(detail);
                }
                self.folds.push(Fold {
                    kind,
                    cell,
                    domain,
                    second: now / SECOND_NS,
                    count: 1,
                    detail: first,
                });
            }
        }
        self.push(now, kind, cell, domain, 1, detail);
    }

    /// Whether a record waits for the host at `now`.
    pub fn pending(&self, now: u64) -> bool {
        !self.bytes.is_empty()
            || self
                .folds
                .iter()
                .any(|f| f.count > 1 && f.second < now / SECOND_NS)
    }

    /// The oldest record not taken, folded counts of past seconds first
    /// put in.
    pub fn take(&mut self, now: u64) -> Option<Record> {
        self.expire(now);
        let len = u16::from_le_bytes([self.bytes.pop_front()?, self.bytes.pop_front()?]) as usize;
        let payload: Vec<u8> = self.bytes.drain(..len).collect();
        // The ring holds only what `push` wrote.
        Record::read(&mut Reader(&payload)).ok()
    }

    /// Puts in the counts of folds whose second is over.
    fn expire(&mut self, now: u64) {
        let second = now / SECOND_NS;
        while let Some(k) = self.folds.iter().position(|f| f.second < second) {
            let f = self.folds.remove(k);
            self.flush(&f, now);
        }
    }

    fn flush(&mut self, f: &Fold, now: u64) {
        if f.count > 1 {
            self.push(now, f.kind, f.cell, f.domain, f.count - 1, &f.detail);
        }
    }

    fn push(&mut self, now: u64, kind: Kind, cell: u32, domain: u32, count: u32, detail: &[u8]) {
        self.seq += 1;
        let size = HEAD + detail.len();
        if self.bytes.capacity() < RING_BYTES
            && self
                .bytes
                .try_reserve_exact(RING_BYTES - self.bytes.len())
                .is_err()
        {
            self.dropped += 1;
            return;
        }
        if self.bytes.len() + 2 + size > RING_BYTES {
            self.dropped += 1;
            return;
        }
        let h = head(
            self.seq,
            now,
            kind,
            cell,
            domain,
            count,
            self.dropped,
            detail.len(),
        );
        self.bytes.extend((size as u16).to_le_bytes());
        self.bytes.extend(h);
        self.bytes.extend(detail.iter().copied());
        self.dropped = 0;
    }
}
