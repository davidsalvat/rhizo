//! Snapshots of cells (spec `snapshots`, design D1–D3 of
//! `add-suspend-resume`, D7 of `add-threads`).
//!
//! A [`Snapshot`] is the encoded byte form itself: capture writes the
//! format directly, and restore reads it in place, so suspending or resuming
//! copies each page exactly once.
//!
//! ```text
//! header   "RHZSNAP1", version u32, machine u16, write-or-execute
//!          ranges u16 (version 9), pages u32, threads u32    (24 bytes)
//! state    brk_start, brk, mmap_next, next thread identifier, then
//!          (version 8) the quotas: memory bytes, CPU ns, CPU ns used,
//!          files (low 32 bits) and sockets (high 32 bits)  (8 x u64)
//! threads  per thread, the current one first: tid, clear_tid, wait kind
//!          (0 runnable, 1 sleep, 2 futex, 3 descriptors), futex address,
//!          futex bitset, time left in ns (u64::MAX: no deadline; version
//!          10: for kinds 0 and 3, that of the call on descriptors the
//!          thread makes again), pc, sp, gpr[31], tls, flags, fp[66]
//!                                                 (107 x u64, 856 bytes each)
//! ranges   per write-or-execute range: start u64, end u64 (version 9)
//! records  per page: virt u64, data offset u64, size u8 (0 = 4 KiB,
//!          1 = 2 MiB), permissions u8 (1 W, 2 USER, 4 NX, 8 write-or-
//!          execute: version 9), 6 zero bytes          (24 bytes each)
//! data     page contents, in record order
//! files    length u64, then the cell's file system as `rhizo_fs` encodes
//!          it (tree, open descriptions and descriptors, pipes, event
//!          counters, `epoll` instances, working directory; version 7:
//!          the pipe of the host's standard input)
//! sockets  length u64, then one record per socket as `rhizo_net` encodes
//!          them (version 6: a connection may be parked in the kernel);
//!          a socket descriptor's index is its record's
//! tree     (version 11) length u64, then the cell's place in the cell tree,
//!          its policies, copy generation, sticky bit and wipe ranges
//!          ([`TreeState`], laid out in `tree.rs`)
//! ```
//!
//! This code writes version 11 and reads version 10 too, through
//! [`upgrade`], which gives a version-10 file the safest tree section
//! (design D20 of `add-cell-hierarchy`): egress and ceiling `none`, the
//! limits of its quotas, no CPU set, an unknown number and generation, and
//! the sticky bit set (it may hold a secret).
//!
//! Every value is little endian; records are sorted by address.
//!
//! The exported form, which leaves the kernel (a file, design D1 of
//! `snapshots-on-disk`), is these bytes followed by their CRC-32C
//! (4 bytes); [`Snapshot::from_bytes`] checks it after the header.
//!
//! In the kernel, a snapshot refers to the pages its cell shared with
//! another (a template, a fork, a loaded image) instead of copying them
//! (design D1-D3 of `add-shared-snapshots`): such a record has 1 in its
//! first reserved byte and the page's physical address as its offset, and
//! no data. The snapshot holds a reference to each of those frames until it
//! is dropped. Those records never leave the kernel: [`Snapshot::export`]
//! writes the pages' contents in their place, and [`Snapshot::from_bytes`]
//! refuses them.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use rhizo_cell::elf::Machine;
use rhizo_cell::quota::Quota;
use rhizo_cell::space::{Frames, WxRanges};
use rhizo_cell::threads::{Thread, Threads, Wait};
use rhizo_cell::{Cell, Registers, USER_END};
use rhizo_fs::Fs;
use rhizo_hal::PhysicalMemory;
use rhizo_mm::FRAME_SIZE;
use rhizo_mm::paging::{EntryFormat, MapError, PageFlags, PageSize, PageTable};
use rhizo_net::Record;

mod tree;
pub use tree::TreeState;

const MAGIC: &[u8; 8] = b"RHZSNAP1";
/// The copy generation of a cell loaded from a version-10 file, which does
/// not hold it (design D22 of `add-cell-hierarchy`): the kernel then gives
/// the cell a value its source cannot have seen.
pub const UNKNOWN_GENERATION: u64 = u64::MAX;
/// Format version written by this code.
pub const VERSION: u32 = 11;
/// The version before it, which this code also reads (design D20 of
/// `add-cell-hierarchy`).
pub const PREVIOUS: u32 = 10;
const HEADER: usize = 24;
const STATE: usize = 8 * 8;
/// Words per thread: 6 of identity and wait, then the registers.
const THREAD_WORDS: usize = 6 + 35 + 66;
const THREAD: usize = THREAD_WORDS * 8;
const RECORD: usize = 24;
/// Bytes of a write-or-execute range: start and end.
const RANGE: usize = 16;
/// The first reserved byte of a record, 1 if it refers to a frame.
const REFERS: usize = 18;
/// "No deadline" in a thread's time left.
const NO_DEADLINE: u64 = u64::MAX;
/// Bytes of the checksum that ends the exported form.
const CHECKSUM: usize = 4;

/// Why bytes are not a snapshot this kernel can resume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatError {
    /// Shorter than its header, state, records or data say.
    Truncated,
    /// Not a Rhizo snapshot.
    BadMagic,
    /// Written by another format version.
    Version(u32),
    /// Taken on another architecture.
    Machine { expected: u16, found: u16 },
    /// The checksum does not match: the bytes were damaged or cut.
    Checksum,
    /// Record `index` is malformed: size, permissions, address, alignment,
    /// order or data offset.
    BadPage { index: usize },
    /// Thread `index` is malformed: identifier (zero, repeated or not below
    /// the next one), wait kind, futex address or bitset; or there is none.
    BadThread { index: usize },
    /// The write-or-execute ranges are unaligned, empty, out of order,
    /// touching, past `USER_END` or too many.
    BadRanges,
    /// The files section does not decode to a consistent file system.
    BadFiles,
    /// The sockets section does not decode, or its records do not match
    /// the socket descriptors one to one.
    BadSockets,
    /// The tree section does not decode: an unknown budget mode or policy,
    /// a malformed rule or label, a CPU weight out of bounds, bad wipe
    /// ranges, or a length unlike its contents.
    BadTree,
    /// Registers the kernel cannot enter safely: a program counter (or, on
    /// x86_64, a thread pointer) outside the cell's addresses, or reserved
    /// `MXCSR` bits, any of which would fault in the kernel itself.
    BadState,
}

/// The plain state of a suspended cell besides its memory and threads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct State {
    pub brk_start: u64,
    pub brk: u64,
    pub mmap_next: u64,
    /// The cell's quotas and the CPU time it used (spec `isolation`).
    pub quota: Quota,
    pub cpu_ns: u64,
}

/// One page of a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page<'a> {
    pub virt: u64,
    pub size: PageSize,
    pub flags: PageFlags,
    pub data: Data<'a>,
}

/// Where the contents of a page of a snapshot are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Data<'a> {
    /// In the snapshot.
    Bytes(&'a [u8]),
    /// In the frame at this physical address, shared with other cells: the
    /// snapshot holds a reference to it.
    Frame(u64),
}

/// Drops one reference to a frame a snapshot refers to.
pub type Release = fn(u64, PageSize);

/// A validated snapshot, in its encoded form.
#[derive(Debug)]
pub struct Snapshot {
    bytes: Vec<u8>,
    /// Records that refer to a frame.
    frames: usize,
    release: Release,
    /// Where the page records start (after the threads and ranges), read
    /// once: every page lookup needs it.
    records: usize,
    /// The identifier of the cell it was taken from in this kernel; 0 for
    /// one read from bytes, which names no cell here.
    origin: u32,
    /// Where the tree section starts (its length), found once: every
    /// resume reads it.
    tree: usize,
}

impl PartialEq for Snapshot {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}

impl Eq for Snapshot {}

impl Drop for Snapshot {
    fn drop(&mut self) {
        if self.frames == 0 {
            return;
        }
        for i in 0..self.page_count() {
            let page = self.page(i);
            if let Data::Frame(phys) = page.data {
                (self.release)(phys, page.size);
            }
        }
    }
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    let mut word = [0u8; 8];
    word.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(word)
}

fn encode_flags(f: PageFlags) -> u8 {
    // A copy-on-write page is writable for the cell; its copy is private.
    (f.contains(PageFlags::WRITABLE) || f.contains(PageFlags::COPY_ON_WRITE)) as u8
        | ((f.contains(PageFlags::USER) as u8) << 1)
        | ((f.contains(PageFlags::NO_EXECUTE) as u8) << 2)
        | ((f.contains(PageFlags::WRITE_OR_EXEC) as u8) << 3)
}

fn decode_flags(b: u8) -> Option<PageFlags> {
    let mut f = PageFlags::USER;
    if b & 2 == 0 || b & !15 != 0 {
        return None; // only user pages, no unknown bits
    }
    if b & 1 != 0 {
        f = f | PageFlags::WRITABLE;
    }
    if b & 4 != 0 {
        f = f | PageFlags::NO_EXECUTE;
    }
    // W^X: writable pages are never executable, and a write-or-execute
    // page is one of the two (writing or running), never neither.
    let wx = b & 8 != 0;
    if wx {
        f = f | PageFlags::WRITE_OR_EXEC;
    }
    let ok =
        !(f.is_writable() && f.is_executable()) && (!wx || f.is_writable() == !f.is_executable());
    ok.then_some(f)
}

/// The words of thread `t` in the format; deadlines become time left after
/// `now` (at least 1 ns, so a due deadline stays a deadline). `kept` is the
/// deadline of the call on descriptors a runnable thread makes again.
fn thread_words(t: &Thread, kept: Option<u64>, now: u64) -> impl Iterator<Item = u64> + '_ {
    let left = |until: u64| until.saturating_sub(now).max(1);
    let (kind, addr, bitset, time) = match t.wait {
        // A thread waiting on descriptors is back at its system-call
        // instruction: it makes its call again with the time it had left
        // (design D3 of `add-sockets-and-poll`, D5 of `snapshots-on-disk`).
        Wait::Ready => (0, 0, 0, kept.map_or(NO_DEADLINE, left)),
        Wait::Io { until } => (3, 0, 0, until.map_or(NO_DEADLINE, left)),
        Wait::Sleep(until) => (1, 0, 0, left(until)),
        Wait::Futex {
            addr,
            bitset,
            until,
        } => (2, addr, bitset as u64, until.map_or(NO_DEADLINE, left)),
    };
    let r = &t.regs;
    [
        t.tid as u64,
        t.clear_tid,
        kind,
        addr,
        bitset,
        time,
        r.pc,
        r.sp,
    ]
    .into_iter()
    .chain(r.gpr)
    .chain([r.tls, r.flags])
    .chain(r.fp.0)
}

impl Snapshot {
    /// Captures `cell` (not running), its write-or-execute and wipe ranges
    /// `wx` and the user half of its page table (its wipe ranges' pages as
    /// zeros); its threads' deadlines, and
    /// `deadlines` (thread, deadline) of the calls on descriptors they make
    /// again, are kept as time left after monotonic time `now`. `files`
    /// refers to its sockets by index in `sockets`.
    ///
    /// Every table of the cell is made its own first, without copying a
    /// page ([`PageTable::unshare_tables`]); then a page other cells share
    /// is referred to, with one more reference from `frames`, and every
    /// other page is copied. The snapshot gives each reference back through
    /// `release` when dropped. Short of frames for the tables, or with a
    /// frame's count full, it copies instead. The cell's memory is left
    /// mapped; the caller frees it.
    #[allow(clippy::too_many_arguments)]
    pub fn capture<F: EntryFormat>(
        machine: Machine,
        cell: &Cell,
        deadlines: &[(u32, u64)],
        wx: &WxRanges,
        files: &Fs,
        sockets: &[Record],
        now: u64,
        mem: &mut impl PhysicalMemory,
        frames: &mut impl Frames,
        release: Release,
    ) -> Self {
        let pt = PageTable::<F>::from_root(cell.root);
        let own = pt.unshare_tables(mem, frames).is_ok();
        let mem = &*mem;
        // Pages of the wipe ranges are captured as zeros: whoever resumes or
        // loads the snapshot gets a copy (design D22 of `add-cell-hierarchy`).
        let wiped = |virt: u64, size: PageSize| {
            let end = virt + size.bytes();
            wx.wiped().iter().any(|&(s, e)| virt < e && s < end)
        };
        let (mut refers, mut data_len) = (Vec::new(), 0usize);
        pt.for_each_lower_leaf(mem, &mut |virt, phys, size, flags| {
            // With every table its own and writable, a writable entry is
            // the frame's only one (copy-on-write invariant): not shared.
            let refer = own
                && !flags.is_writable()
                && !wiped(virt, size)
                && frames.is_shared(phys)
                && frames.try_share(phys);
            if !refer {
                data_len += size.bytes() as usize;
            }
            refers.push(refer);
        });
        let (pages, threads) = (refers.len(), cell.threads.len());
        let ranges = wx.ranges();
        let data_start = HEADER + STATE + threads * THREAD + ranges.len() * RANGE + pages * RECORD;
        let mut encoded = Vec::new();
        files.encode(&mut encoded);
        let mut records = Vec::new();
        rhizo_net::encode_records(sockets, &mut records);
        // Its policies are the defaults until the kernel sets its node's
        // (`set_tree`).
        let mut tree = Vec::new();
        tree::encode(
            &TreeState {
                spec: rhizo_cap::Spec {
                    limits: limits_of(&cell.quota),
                    ..rhizo_cap::Spec::default()
                },
                pid: cell.pid,
                generation: cell.generation,
                exposed: cell.exposed,
                wiped: wx.wiped().to_vec(),
            },
            &mut tree,
        );
        // Allocated zeroed at its final size, then filled in place: no copy
        // of the pages when it grows, and no zeroing loop, which unoptimized
        // builds run a byte at a time.
        let data_end = data_start + data_len;
        let mut b = vec![0u8; data_end + 16 + encoded.len() + records.len() + tree.len()];
        let mut rec = 0;
        let mut put = |bytes: &[u8]| {
            b[rec..rec + bytes.len()].copy_from_slice(bytes);
            rec += bytes.len();
        };
        put(MAGIC);
        put(&VERSION.to_le_bytes());
        put(&(machine as u16).to_le_bytes());
        // At most 64 ranges (`WxRanges`).
        put(&(ranges.len() as u16).to_le_bytes());
        put(&(pages as u32).to_le_bytes());
        put(&(threads as u32).to_le_bytes());
        let state = [
            cell.brk_start,
            cell.brk,
            cell.mmap_next,
            cell.threads.next_tid() as u64,
            cell.quota.memory,
            cell.quota.cpu_ns,
            cell.cpu_ns,
            cell.quota.files as u64 | (cell.quota.sockets as u64) << 32,
        ];
        let kept = |tid| deadlines.iter().find(|d| d.0 == tid).map(|d| d.1);
        let words = cell
            .threads
            .iter()
            .flat_map(|t| thread_words(t, kept(t.tid), now));
        let ranges = ranges.iter().flat_map(|&(start, end)| [start, end]);
        for v in state.into_iter().chain(words).chain(ranges) {
            put(&v.to_le_bytes());
        }
        // Records and data in one walk: both places are known now.
        let (mut offset, mut i) = (0u64, 0);
        pt.for_each_lower_leaf(mem, &mut |virt, phys, size, flags| {
            let r = &mut b[rec..rec + RECORD];
            r[..8].copy_from_slice(&virt.to_le_bytes());
            if refers[i] {
                r[8..16].copy_from_slice(&phys.to_le_bytes());
                r[REFERS] = 1;
            } else {
                r[8..16].copy_from_slice(&offset.to_le_bytes());
                let (at, n) = (data_start + offset as usize, size.bytes() as usize);
                if !wiped(virt, size) {
                    mem.read_pages(phys, &mut b[at..at + n]);
                }
                offset += size.bytes();
            }
            b[rec + 16] = (size == PageSize::Large) as u8;
            b[rec + 17] = encode_flags(flags);
            rec += RECORD;
            i += 1;
        });
        let mut tail = data_end;
        for part in [&encoded, &records] {
            b[tail..tail + 8].copy_from_slice(&(part.len() as u64).to_le_bytes());
            b[tail + 8..tail + 8 + part.len()].copy_from_slice(part);
            tail += 8 + part.len();
        }
        b[tail..].copy_from_slice(&tree); // with its length
        Self {
            bytes: b,
            frames: refers.iter().filter(|&&r| r).count(),
            release,
            records: data_start - pages * RECORD,
            origin: cell.id,
            tree: tail,
        }
    }

    /// The snapshot as bytes that stand alone, to leave the kernel: the
    /// contents of the frames it refers to are written in their place
    /// (design D2 of `add-shared-snapshots`), then the checksum (D1 of
    /// `snapshots-on-disk`), computed by `crc` (a [`Crc32c`]). The snapshot
    /// keeps its references.
    pub fn export(&self, mem: &impl PhysicalMemory, crc: Crc32c) -> Vec<u8> {
        let mut b = self.standalone(mem);
        let sum = crc(&b);
        b.extend_from_slice(&sum.to_le_bytes());
        b
    }

    /// The snapshot's bytes with its frames' contents in place.
    fn standalone(&self, mem: &impl PhysicalMemory) -> Vec<u8> {
        if self.frames == 0 {
            let mut b = Vec::with_capacity(self.bytes.len() + CHECKSUM);
            b.extend_from_slice(&self.bytes);
            return b;
        }
        let (records, pages) = (self.records(), self.page_count());
        let data_len: u64 = (0..pages).map(|i| self.page(i).size.bytes()).sum();
        let tail = &self.bytes[self.files_at()..];
        let mut b = Vec::with_capacity(
            records + pages * RECORD + data_len as usize + tail.len() + CHECKSUM,
        );
        b.extend_from_slice(&self.bytes[..records]);
        let mut offset = 0u64;
        for i in 0..pages {
            let rec = &self.bytes[records + i * RECORD..][..RECORD];
            b.extend_from_slice(&rec[..8]);
            b.extend_from_slice(&offset.to_le_bytes());
            b.extend_from_slice(&rec[16..REFERS]);
            b.extend_from_slice(&[0; 6]);
            offset += self.page(i).size.bytes();
        }
        for i in 0..pages {
            match self.page(i).data {
                Data::Bytes(data) => b.extend_from_slice(data),
                Data::Frame(phys) => {
                    let at = b.len();
                    b.resize(at + self.page(i).size.bytes() as usize, 0);
                    mem.read_pages(phys, &mut b[at..]);
                }
            }
        }
        b.extend_from_slice(tail);
        b
    }

    /// Bytes the snapshot takes in the kernel heap.
    pub fn heap_bytes(&self) -> usize {
        self.bytes.len()
    }

    /// Bytes of the frames it refers to.
    pub fn shared_bytes(&self) -> u64 {
        (0..self.page_count())
            .map(|i| self.page(i))
            .filter(|p| matches!(p.data, Data::Frame(_)))
            .map(|p| p.size.bytes())
            .sum()
    }

    /// Validates `bytes`, the exported form of a snapshot, for `machine`:
    /// its header, then its checksum (computed by `crc`, a [`Crc32c`]), then
    /// everything it holds.
    pub fn from_bytes(
        mut bytes: Vec<u8>,
        machine: Machine,
        crc: Crc32c,
    ) -> Result<Self, FormatError> {
        if bytes.len() < HEADER + STATE + CHECKSUM {
            return Err(FormatError::Truncated);
        }
        if &bytes[..8] != MAGIC {
            return Err(FormatError::BadMagic);
        }
        let version = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        if version == PREVIOUS {
            bytes = upgrade(bytes, crc)?;
        } else if version != VERSION {
            return Err(FormatError::Version(version));
        }
        let found = u16::from_le_bytes([bytes[12], bytes[13]]);
        if found != machine as u16 {
            return Err(FormatError::Machine {
                expected: machine as u16,
                found,
            });
        }
        let end = bytes.len() - CHECKSUM;
        let mut sum = [0; CHECKSUM];
        sum.copy_from_slice(&bytes[end..]);
        if crc(&bytes[..end]) != u32::from_le_bytes(sum) {
            return Err(FormatError::Checksum);
        }
        bytes.truncate(end);
        if bytes.len() < HEADER + STATE {
            return Err(FormatError::Truncated);
        }
        let mut snapshot = Self {
            bytes,
            frames: 0,
            release: |_, _| {},
            records: 0,
            origin: 0,
            tree: 0,
        };
        let pages = snapshot.page_count();
        // Counts are 32-bit: these products cannot overflow a 64-bit usize.
        let records = snapshot.ranges_at() + snapshot.range_count() * RANGE;
        snapshot.records = records;
        let data_start = records + pages * RECORD;
        if snapshot.bytes.len() < data_start {
            return Err(FormatError::Truncated);
        }
        snapshot.check_threads(machine)?;
        if WxRanges::from_ranges(snapshot.range_list()).is_none() {
            return Err(FormatError::BadRanges);
        }
        let mut expected_offset = 0u64;
        let mut prev_end = 0u64;
        for index in 0..pages {
            let rec = records + index * RECORD;
            let b = &snapshot.bytes;
            let (virt, offset, size, flags) =
                (u64_at(b, rec), u64_at(b, rec + 8), b[rec + 16], b[rec + 17]);
            let bad = FormatError::BadPage { index };
            let size = match size {
                0 => PageSize::Small,
                1 => PageSize::Large,
                _ => return Err(bad),
            };
            if decode_flags(flags).is_none()
                || b[rec + 18..rec + 24] != [0; 6]
                || !virt.is_multiple_of(size.bytes())
                || virt < prev_end
                || virt
                    .checked_add(size.bytes())
                    .is_none_or(|end| end > USER_END)
                || offset != expected_offset
            {
                return Err(bad);
            }
            prev_end = virt + size.bytes();
            expected_offset += size.bytes();
        }
        let files_at = data_start as u64 + expected_offset;
        let len = snapshot.bytes.len() as u64;
        if len < files_at + 8 {
            return Err(FormatError::Truncated);
        }
        let sockets_at = u64_at(&snapshot.bytes, files_at as usize)
            .checked_add(files_at + 8)
            .ok_or(FormatError::Truncated)?;
        if len < sockets_at.saturating_add(8) {
            return Err(FormatError::Truncated);
        }
        let tree_at = u64_at(&snapshot.bytes, sockets_at as usize)
            .checked_add(sockets_at + 8)
            .ok_or(FormatError::Truncated)?;
        if len < tree_at.saturating_add(8)
            || len - tree_at - 8 != u64_at(&snapshot.bytes, tree_at as usize)
        {
            return Err(FormatError::Truncated);
        }
        tree::decode(&snapshot.bytes[tree_at as usize + 8..]).ok_or(FormatError::BadTree)?;
        snapshot.tree = tree_at as usize;
        let fs = Fs::decode(snapshot.files_bytes()).map_err(|_| FormatError::BadFiles)?;
        let records = rhizo_net::decode_records(snapshot.sockets_bytes())
            .map_err(|_| FormatError::BadSockets)?;
        // Each record is held by exactly one description.
        let mut held = alloc::vec![false; records.len()];
        for i in fs.sockets() {
            match held.get_mut(i as usize) {
                Some(h) if !*h => *h = true,
                _ => return Err(FormatError::BadSockets),
            }
        }
        if held.contains(&false) {
            return Err(FormatError::BadSockets);
        }
        Ok(snapshot)
    }

    fn u32_at(&self, at: usize) -> usize {
        u32::from_le_bytes([
            self.bytes[at],
            self.bytes[at + 1],
            self.bytes[at + 2],
            self.bytes[at + 3],
        ]) as usize
    }

    fn page_count(&self) -> usize {
        self.u32_at(16)
    }

    fn thread_count(&self) -> usize {
        self.u32_at(20)
    }

    fn range_count(&self) -> usize {
        u16::from_le_bytes([self.bytes[14], self.bytes[15]]) as usize
    }

    /// Where the write-or-execute ranges start.
    fn ranges_at(&self) -> usize {
        HEADER + STATE + self.thread_count() * THREAD
    }

    /// Where the page records start.
    fn records(&self) -> usize {
        self.records
    }

    /// The write-or-execute ranges as stored (the size was checked).
    fn range_list(&self) -> Vec<(u64, u64)> {
        let at = self.ranges_at();
        (0..self.range_count())
            .map(|i| {
                (
                    u64_at(&self.bytes, at + i * RANGE),
                    u64_at(&self.bytes, at + i * RANGE + 8),
                )
            })
            .collect()
    }

    /// The cell's write-or-execute ranges (design D7 of `node-jit`) and
    /// wipe ranges (design D22 of `add-cell-hierarchy`).
    pub fn wx(&self) -> WxRanges {
        // Validated by `from_bytes` or written by `capture`.
        let wx = if self.range_count() == 0 {
            WxRanges::default()
        } else {
            WxRanges::from_ranges(self.range_list()).unwrap_or_default()
        };
        let wiped = tree::wiped(&self.bytes[self.tree_at() + 8..]);
        if wiped.is_empty() {
            return wx;
        }
        wx.with_wiped(wiped).unwrap_or_default()
    }

    /// Validates the threads (`from_bytes`; the size was checked).
    fn check_threads(&self, machine: Machine) -> Result<(), FormatError> {
        let next_tid = u64_at(&self.bytes, HEADER + 24);
        if self.thread_count() == 0 {
            return Err(FormatError::BadThread { index: 0 });
        }
        let mut tids = Vec::with_capacity(self.thread_count());
        for index in 0..self.thread_count() {
            let w = |i: usize| u64_at(&self.bytes, HEADER + STATE + index * THREAD + i * 8);
            let (tid, kind, addr, bitset, time) = (w(0), w(2), w(3), w(4), w(5));
            let wait_ok = match kind {
                0 | 3 => addr == 0 && bitset == 0,
                1 => addr == 0 && bitset == 0 && time != NO_DEADLINE,
                2 => {
                    addr.is_multiple_of(4)
                        && addr < USER_END
                        && bitset != 0
                        && bitset <= u32::MAX as u64
                }
                _ => false,
            };
            if !wait_ok || tid == 0 || tid >= next_tid || next_tid > 1 << 31 || tids.contains(&tid)
            {
                return Err(FormatError::BadThread { index });
            }
            // Registers that would fault in the kernel on entry: a program
            // counter (or, on x86_64, a thread pointer) outside the cell's
            // addresses, or reserved MXCSR bits. MXCSR is bytes 24-27 of the
            // FXSAVE image (bits 16-31 reserved); bytes 28-31 are
            // MXCSR_MASK, which FXSAVE fills and FXRSTOR ignores.
            let (pc, tls, mxcsr) = (w(6), w(39), w(41 + 3) as u32);
            let x86 = machine == Machine::X86_64;
            if pc >= USER_END || x86 && (tls >= USER_END || mxcsr >> 16 != 0) {
                return Err(FormatError::BadState);
            }
            tids.push(tid);
        }
        Ok(())
    }

    /// The cell's plain state besides its threads.
    pub fn state(&self) -> State {
        let w = |i: usize| u64_at(&self.bytes, HEADER + i * 8);
        State {
            brk_start: w(0),
            brk: w(1),
            mmap_next: w(2),
            quota: Quota {
                memory: w(4),
                cpu_ns: w(5),
                files: w(7) as u32,
                sockets: (w(7) >> 32) as u32,
            },
            cpu_ns: w(6),
        }
    }

    /// Lowers the cell's quotas to at most `most`, field by field: a
    /// snapshot from outside never gets more than the host gives a new cell
    /// (spec `isolation`, design D4 of `snapshots-on-disk`).
    pub fn cap_quota(&mut self, most: Quota) {
        let q = self.state().quota;
        let words = [
            (4, q.memory.min(most.memory)),
            (5, q.cpu_ns.min(most.cpu_ns)),
            (
                7,
                q.files.min(most.files) as u64 | (q.sockets.min(most.sockets) as u64) << 32,
            ),
        ];
        for (i, v) in words {
            let at = HEADER + i * 8;
            self.bytes[at..at + 8].copy_from_slice(&v.to_le_bytes());
        }
    }

    /// The cell's threads, their deadlines taken as time left after
    /// monotonic time `now`.
    pub fn threads(&self, now: u64) -> Threads {
        let thread = |index: usize| {
            let w = |i: usize| u64_at(&self.bytes, HEADER + STATE + index * THREAD + i * 8);
            let until = |left: u64| (left != NO_DEADLINE).then(|| now.saturating_add(left));
            let wait = match w(2) {
                1 => Wait::Sleep(until(w(5)).unwrap_or(now)),
                2 => Wait::Futex {
                    addr: w(3),
                    bitset: w(4) as u32,
                    until: until(w(5)),
                },
                3 => Wait::Io { until: until(w(5)) },
                _ => Wait::Ready,
            };
            let mut regs = Registers {
                pc: w(6),
                sp: w(7),
                tls: w(39),
                flags: w(40),
                ..Registers::default()
            };
            for (i, g) in regs.gpr.iter_mut().enumerate() {
                *g = w(8 + i);
            }
            for (i, f) in regs.fp.0.iter_mut().enumerate() {
                *f = w(41 + i);
            }
            Thread {
                tid: w(0) as u32,
                regs,
                clear_tid: w(1),
                wait,
            }
        };
        let next_tid = u64_at(&self.bytes, HEADER + 24) as u32;
        // Validated by `from_bytes` or written by `capture`: never empty.
        Threads::from_parts((0..self.thread_count()).map(thread).collect(), next_tid)
            .unwrap_or_else(|| Threads::new(Thread::new(1, Registers::default())))
    }

    /// The deadlines of the calls on descriptors the threads make again
    /// (thread, deadline), taken as time left after monotonic time `now`.
    pub fn deadlines(&self, now: u64) -> Vec<(u32, u64)> {
        (0..self.thread_count())
            .filter_map(|index| {
                let w = |i: usize| u64_at(&self.bytes, HEADER + STATE + index * THREAD + i * 8);
                let calls = matches!(w(2), 0 | 3) && w(5) != NO_DEADLINE;
                calls.then(|| (w(0) as u32, now.saturating_add(w(5))))
            })
            .collect()
    }

    /// Where the files section starts (its length).
    fn files_at(&self) -> usize {
        let pages = self.page_count();
        let data_start = self.records() + pages * RECORD;
        // Pages are stored in record order: the data ends after the last
        // one stored (the last one, unless some refer to frames).
        let rec = |i: usize| self.records() + i * RECORD;
        let last = match self.frames {
            0 => pages.checked_sub(1),
            _ => (0..pages).rev().find(|&i| self.bytes[rec(i) + REFERS] == 0),
        };
        let data_len = last.map(|i| {
            let size = match self.bytes[rec(i) + 16] {
                1 => PageSize::Large,
                _ => PageSize::Small,
            };
            u64_at(&self.bytes, rec(i) + 8) as usize + size.bytes() as usize
        });
        data_start + data_len.unwrap_or(0)
    }

    /// The encoded file system (after the page data and its length).
    pub fn files_bytes(&self) -> &[u8] {
        let at = self.files_at();
        &self.bytes[at + 8..at + 8 + u64_at(&self.bytes, at) as usize]
    }

    /// Where the sockets section starts (its length).
    fn sockets_at(&self) -> usize {
        let at = self.files_at();
        at + 8 + u64_at(&self.bytes, at) as usize
    }

    /// The encoded socket records (after the file system and their length).
    fn sockets_bytes(&self) -> &[u8] {
        let at = self.sockets_at();
        &self.bytes[at + 8..at + 8 + u64_at(&self.bytes, at) as usize]
    }

    /// Where the tree section starts (its length).
    fn tree_at(&self) -> usize {
        self.tree
    }

    /// The cell's place in the tree, policies, copy generation, sticky bit
    /// and wipe ranges (format version 11).
    pub fn tree(&self) -> TreeState {
        // Validated by `from_bytes` or written by `capture`.
        tree::decode(&self.bytes[self.tree_at() + 8..]).unwrap_or_else(|| TreeState {
            spec: rhizo_cap::Spec::default(),
            pid: 0,
            generation: UNKNOWN_GENERATION,
            exposed: true,
            wiped: Vec::new(),
        })
    }

    /// Records `spec`, the policies and limits of the cell's node, in place
    /// of the defaults [`capture`](Self::capture) wrote. The section is
    /// last: no page moves.
    pub fn set_tree(&mut self, spec: &rhizo_cap::Spec) {
        let state = TreeState {
            spec: spec.clone(),
            ..self.tree()
        };
        self.bytes.truncate(self.tree_at());
        tree::encode(&state, &mut self.bytes);
    }

    /// What a cell loaded from these bytes may have under a parent whose
    /// egress and ceiling are `egress` and `ceiling` (the host's choice for
    /// a root) and whose budget leaves `most` (spec `snapshots`, loaded
    /// snapshots hold no authority): its egress and ceiling intersected
    /// with those, its limits at most `most`, no CPU set; its label, CPU
    /// weight and cap and its policy for when its parent ends as written.
    pub fn placed(
        &self,
        egress: &rhizo_cap::Egress,
        ceiling: &rhizo_cap::Egress,
        most: rhizo_cap::Limits,
    ) -> Result<rhizo_cap::Spec, rhizo_cap::Error> {
        let spec = self.tree().spec;
        let l = spec.limits;
        let own = spec.ceiling.as_ref().unwrap_or(&spec.egress);
        Ok(rhizo_cap::Spec {
            limits: rhizo_cap::Limits {
                memory: l.memory.min(most.memory),
                cpu_ns: l.cpu_ns.min(most.cpu_ns),
                sockets: l.sockets.min(most.sockets),
                cells: l.cells.min(most.cells),
            },
            ceiling: Some(own.intersect(ceiling)?),
            egress: spec.egress.intersect(egress)?,
            cpus: 0,
            ..spec
        })
    }

    /// The records of the cell's sockets, in the order its descriptors
    /// refer to them.
    pub fn sockets(&self) -> Vec<Record> {
        // Validated by `from_bytes` or written by `capture`.
        rhizo_net::decode_records(self.sockets_bytes()).unwrap_or_default()
    }

    /// The cell's file system, open descriptors and working directory.
    pub fn files(&self) -> Fs {
        // Validated by `from_bytes` or written by `capture`.
        Fs::decode_trusted(self.files_bytes()).unwrap_or_default()
    }

    /// Number of pages.
    pub fn pages(&self) -> usize {
        self.page_count()
    }

    /// Page `index` (records were validated by `from_bytes` or written by
    /// `capture`).
    pub fn page(&self, index: usize) -> Page<'_> {
        let rec = self.records() + index * RECORD;
        let b = &self.bytes;
        let size = if b[rec + 16] == 1 {
            PageSize::Large
        } else {
            PageSize::Small
        };
        let data = if b[rec + REFERS] == 1 {
            Data::Frame(u64_at(b, rec + 8))
        } else {
            let start = self.records() + self.page_count() * RECORD + u64_at(b, rec + 8) as usize;
            Data::Bytes(&b[start..start + size.bytes() as usize])
        };
        Page {
            virt: u64_at(b, rec),
            size,
            flags: decode_flags(b[rec + 17]).unwrap_or(PageFlags::USER),
            data,
        }
    }

    /// Index of the page containing user address `addr`, if any.
    pub fn find(&self, addr: u64) -> Option<usize> {
        let (mut lo, mut hi) = (0, self.page_count());
        while lo < hi {
            let mid = (lo + hi) / 2;
            let p = self.page(mid);
            if addr < p.virt {
                hi = mid;
            } else if addr >= p.virt + p.size.bytes() {
                lo = mid + 1;
            } else {
                return Some(mid);
            }
        }
        None
    }

    /// Maps page `index` into `pt`: a frame the snapshot refers to is
    /// shared once more, copy-on-write if the page is writable; any other
    /// page goes into a fresh frame holding its contents (as does a shared
    /// one whose count is full). A 2 MiB page comes back as 512 pages of
    /// 4 KiB when no 2 MiB run of frames is free (as when it was first
    /// mapped).
    pub fn restore_page<F: EntryFormat>(
        &self,
        index: usize,
        pt: &PageTable<F>,
        mem: &mut impl PhysicalMemory,
        frames: &mut impl Frames,
    ) -> Result<(), MapError> {
        let page = self.page(index);
        if let Data::Frame(phys) = page.data
            && frames.try_share(phys)
        {
            let flags = match page.flags.is_writable() {
                true => page.flags | PageFlags::COPY_ON_WRITE,
                false => page.flags,
            };
            return pt
                .map(
                    mem,
                    &mut || frames.alloc(),
                    page.virt,
                    phys,
                    page.size,
                    flags,
                )
                .inspect_err(|_| frames.free(phys, page.size));
        }
        let large = match page.size {
            PageSize::Large => frames.alloc_large(),
            _ => None,
        };
        let pieces = match large {
            Some(phys) => alloc::vec![(0, phys, page.size)],
            None => (0..page.size.bytes())
                .step_by(FRAME_SIZE as usize)
                .map(|at| (at, 0, PageSize::Small))
                .collect(),
        };
        for (at, phys, size) in pieces {
            let phys = match size {
                PageSize::Small => frames.alloc().ok_or(MapError::OutOfFrames)?,
                _ => phys,
            };
            let n = size.bytes();
            match page.data {
                Data::Bytes(data) => mem.write_pages(phys, &data[at as usize..(at + n) as usize]),
                Data::Frame(from) => mem.copy(from + at, phys, n),
            }
            let virt = page.virt + at;
            pt.map(mem, &mut || frames.alloc(), virt, phys, size, page.flags)
                .inspect_err(|_| frames.free(phys, size))?;
        }
        Ok(())
    }

    /// The identifier of the cell this snapshot was taken from in this
    /// kernel, 0 if it was read from bytes: a snapshot from bytes refers to
    /// no cell of this kernel (spec `snapshots`, loaded snapshots hold no
    /// authority).
    pub fn origin(&self) -> u32 {
        self.origin
    }

    /// A cell `id` with root `root` (an empty user half, sharing the kernel
    /// half as the architecture needs) in the snapshot's state, its threads'
    /// deadlines counted from monotonic time `now`, its number in its domain
    /// as captured. Its copy generation is
    /// the captured cell's ([`UNKNOWN_GENERATION`] for a version-10 file):
    /// the kernel counts the resume as a copy.
    pub fn cell(&self, id: u32, root: u64, now: u64) -> Cell {
        let s = self.state();
        let (pid, generation, exposed) = tree::copy_state(&self.bytes[self.tree_at() + 8..]);
        Cell {
            id,
            // Its number as it was (unknown in a version-10 file); the
            // kernel gives a resumed or loaded cell a new one.
            pid: if pid == 0 { id } else { pid },
            domain: 0,
            root,
            threads: self.threads(now),
            brk_start: s.brk_start,
            brk: s.brk,
            mmap_next: s.mmap_next,
            exit_status: None,
            quota: s.quota,
            cpu_ns: s.cpu_ns,
            generation,
            exposed,
        }
    }
}

/// A cell's tree limits from its quotas (no limit on descendants).
fn limits_of(q: &Quota) -> rhizo_cap::Limits {
    rhizo_cap::Limits {
        memory: q.memory,
        cpu_ns: q.cpu_ns,
        sockets: q.sockets,
        cells: u32::MAX,
    }
}

/// Converts `bytes`, a snapshot file of any version this code converts,
/// into the current version and checks the result as a kernel of the
/// architecture it names would (`rhizo snapshot upgrade`, design D20 of
/// `add-cell-hierarchy`); returns the bytes to write.
pub fn upgrade_file(bytes: Vec<u8>, crc: Crc32c) -> Result<Vec<u8>, FormatError> {
    let bytes = upgrade(bytes, crc)?;
    let machine = match u16::from_le_bytes([bytes[12], bytes[13]]) {
        183 => Machine::Aarch64,
        62 => Machine::X86_64,
        found => {
            return Err(FormatError::Machine {
                expected: Machine::X86_64 as u16,
                found,
            });
        }
    };
    Snapshot::from_bytes(bytes.clone(), machine, crc)?;
    Ok(bytes)
}

/// Converts the exported form of a snapshot of the previous format version
/// into the current one, checksum included (design D20 of
/// `add-cell-hierarchy`); a current one comes back as it is. The bytes are
/// checked as far as the conversion needs (magic, version, checksum, the
/// quota words); [`Snapshot::from_bytes`] checks the rest. A version-10
/// file gets the safest tree section: the limits of its quotas, egress and
/// ceiling `none`, no CPU set, an unknown number and generation, the sticky
/// bit set. Deterministic: the same file always converts to the same bytes.
pub fn upgrade(mut bytes: Vec<u8>, crc: Crc32c) -> Result<Vec<u8>, FormatError> {
    if bytes.len() < HEADER + STATE + CHECKSUM {
        return Err(FormatError::Truncated);
    }
    if &bytes[..8] != MAGIC {
        return Err(FormatError::BadMagic);
    }
    match u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) {
        VERSION => return Ok(bytes),
        PREVIOUS => {}
        other => return Err(FormatError::Version(other)),
    }
    let end = bytes.len() - CHECKSUM;
    let mut sum = [0; CHECKSUM];
    sum.copy_from_slice(&bytes[end..]);
    if crc(&bytes[..end]) != u32::from_le_bytes(sum) {
        return Err(FormatError::Checksum);
    }
    bytes.truncate(end);
    let w = |i: usize| u64_at(&bytes, HEADER + i * 8);
    let quota = Quota {
        memory: w(4),
        cpu_ns: w(5),
        files: w(7) as u32,
        sockets: (w(7) >> 32) as u32,
    };
    tree::encode(
        &TreeState {
            spec: rhizo_cap::Spec {
                limits: limits_of(&quota),
                ..rhizo_cap::Spec::default()
            },
            pid: 0,
            generation: UNKNOWN_GENERATION,
            exposed: true,
            wiped: Vec::new(),
        },
        &mut bytes,
    );
    bytes[8..12].copy_from_slice(&VERSION.to_le_bytes());
    let sum = crc(&bytes);
    bytes.extend_from_slice(&sum.to_le_bytes());
    Ok(bytes)
}

/// A function computing the CRC-32C of exported snapshots: [`crc32c`], or
/// one with the same results that uses the CPU's CRC instructions, which
/// the kernel passes in (change `fast-mem-primitives`).
pub type Crc32c = fn(&[u8]) -> u32;

/// CRC-32C (Castagnoli, reflected, as iSCSI and ext4 use it) of `bytes`,
/// eight bytes at a time ("slicing by 8"): the portable form, and the
/// reference the kernel's accelerated one is tested against.
pub fn crc32c(bytes: &[u8]) -> u32 {
    const TABLES: [[u32; 256]; 8] = {
        let mut t = [[0u32; 256]; 8];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut bit = 0;
            while bit < 8 {
                c = if c & 1 != 0 {
                    c >> 1 ^ 0x82f6_3b78
                } else {
                    c >> 1
                };
                bit += 1;
            }
            t[0][i] = c;
            i += 1;
        }
        let mut k = 1;
        while k < 8 {
            let mut i = 0;
            while i < 256 {
                let c = t[k - 1][i];
                t[k][i] = c >> 8 ^ t[0][(c & 0xff) as usize];
                i += 1;
            }
            k += 1;
        }
        t
    };
    let byte = |c: u32, b: u8| TABLES[0][((c ^ b as u32) & 0xff) as usize] ^ c >> 8;
    let mut words = bytes.chunks_exact(8);
    let mut c = !0u32;
    for w in &mut words {
        let mut word = [0; 8];
        word.copy_from_slice(w);
        let v = u64::from_le_bytes(word) ^ c as u64;
        c = (0..8).fold(0, |acc, k| {
            acc ^ TABLES[7 - k][(v >> (8 * k) & 0xff) as usize]
        });
    }
    !words.remainder().iter().fold(c, |c, &b| byte(c, b))
}

#[cfg(test)]
mod tests;
