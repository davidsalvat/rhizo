//! Snapshots of cells (spec `snapshots`, design D1–D3 of
//! `add-suspend-resume`, D7 of `add-threads`).
//!
//! A [`Snapshot`] is the encoded byte form itself: capture writes the
//! format directly, and restore reads it in place, so suspending or resuming
//! copies each page exactly once.
//!
//! ```text
//! header   "RHZSNAP1", version u32, machine u16, reserved u16, pages u32,
//!          threads u32                                       (24 bytes)
//! state    brk_start, brk, mmap_next, next thread identifier  (4 x u64)
//! threads  per thread, the current one first: tid, clear_tid, wait kind
//!          (0 runnable, 1 sleep, 2 futex), futex address, futex bitset,
//!          time left in ns (u64::MAX: no deadline), pc, sp, gpr[31], tls,
//!          flags, fp[66]                          (107 x u64, 856 bytes each)
//! records  per page: virt u64, data offset u64, size u8 (0 = 4 KiB,
//!          1 = 2 MiB), permissions u8 (1 W, 2 USER, 4 NX), 6 zero bytes
//!                                                     (24 bytes each)
//! data     page contents, in record order
//! files    length u64, then the cell's file system as `rhizo_fs` encodes
//!          it (tree, open descriptions and descriptors, working directory)
//! ```
//!
//! Every value is little endian; records are sorted by address.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec::Vec;
use rhizo_cell::elf::Machine;
use rhizo_cell::space::Frames;
use rhizo_cell::threads::{Thread, Threads, Wait};
use rhizo_cell::{Cell, Registers};
use rhizo_fs::Fs;
use rhizo_hal::PhysicalMemory;
use rhizo_mm::paging::{EntryFormat, MapError, PageFlags, PageSize, PageTable};

const MAGIC: &[u8; 8] = b"RHZSNAP1";
/// Format version written by this code.
pub const VERSION: u32 = 4;
const HEADER: usize = 24;
const STATE: usize = 4 * 8;
/// Words per thread: 6 of identity and wait, then the registers.
const THREAD_WORDS: usize = 6 + 35 + 66;
const THREAD: usize = THREAD_WORDS * 8;
const RECORD: usize = 24;
/// "No deadline" in a thread's time left.
const NO_DEADLINE: u64 = u64::MAX;
/// Highest user address + 1.
const USER_END: u64 = 0x0000_8000_0000_0000;

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
    /// Record `index` is malformed: size, permissions, address, alignment,
    /// order or data offset.
    BadPage { index: usize },
    /// Thread `index` is malformed: identifier (zero, repeated or not below
    /// the next one), wait kind, futex address or bitset; or there is none.
    BadThread { index: usize },
    /// The files section does not decode to a consistent file system.
    BadFiles,
}

/// The plain state of a suspended cell besides its memory and threads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct State {
    pub brk_start: u64,
    pub brk: u64,
    pub mmap_next: u64,
}

/// One page of a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page<'a> {
    pub virt: u64,
    pub size: PageSize,
    pub flags: PageFlags,
    pub data: &'a [u8],
}

/// A validated snapshot, in its encoded form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    bytes: Vec<u8>,
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
}

fn decode_flags(b: u8) -> Option<PageFlags> {
    let mut f = PageFlags::USER;
    if b & 2 == 0 || b & !7 != 0 {
        return None; // only user pages, no unknown bits
    }
    if b & 1 != 0 {
        f = f | PageFlags::WRITABLE;
    }
    if b & 4 != 0 {
        f = f | PageFlags::NO_EXECUTE;
    }
    // W^X: writable pages are never executable.
    (!(f.is_writable() && f.is_executable())).then_some(f)
}

/// The words of thread `t` in the format; deadlines become time left after
/// `now` (at least 1 ns, so a due deadline stays a deadline).
fn thread_words(t: &Thread, now: u64) -> impl Iterator<Item = u64> + '_ {
    let left = |until: u64| until.saturating_sub(now).max(1);
    let (kind, addr, bitset, time) = match t.wait {
        Wait::Ready => (0, 0, 0, NO_DEADLINE),
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
    /// Captures `cell` (not running) and the user half of its page table,
    /// copying every page; its threads' deadlines are kept as time left
    /// after monotonic time `now`. The cell's memory is left as it was; the
    /// caller frees it.
    pub fn capture<F: EntryFormat>(
        machine: Machine,
        cell: &Cell,
        files: &Fs,
        now: u64,
        mem: &impl PhysicalMemory,
    ) -> Self {
        let pt = PageTable::<F>::from_root(cell.root);
        let (mut pages, mut data_len) = (0usize, 0usize);
        pt.for_each_lower_leaf(mem, &mut |_, _, size, _| {
            pages += 1;
            data_len += size.bytes() as usize;
        });
        let threads = cell.threads.len();
        let data_start = HEADER + STATE + threads * THREAD + pages * RECORD;
        let mut encoded = Vec::new();
        files.encode(&mut encoded);
        // Sized once: growing past the pages would copy them all again.
        let mut b = Vec::with_capacity(data_start + data_len + 8 + encoded.len());
        b.extend_from_slice(MAGIC);
        b.extend_from_slice(&VERSION.to_le_bytes());
        b.extend_from_slice(&(machine as u16).to_le_bytes());
        b.extend_from_slice(&[0, 0]);
        b.extend_from_slice(&(pages as u32).to_le_bytes());
        b.extend_from_slice(&(threads as u32).to_le_bytes());
        let state = [
            cell.brk_start,
            cell.brk,
            cell.mmap_next,
            cell.threads.next_tid() as u64,
        ];
        let words = cell.threads.iter().flat_map(|t| thread_words(t, now));
        for v in state.into_iter().chain(words) {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let mut offset = 0u64;
        pt.for_each_lower_leaf(mem, &mut |virt, _, size, flags| {
            b.extend_from_slice(&virt.to_le_bytes());
            b.extend_from_slice(&offset.to_le_bytes());
            b.push((size == PageSize::Large) as u8);
            b.push(encode_flags(flags));
            b.extend_from_slice(&[0; 6]);
            offset += size.bytes();
        });
        b.resize(data_start + data_len, 0);
        let mut at = data_start;
        pt.for_each_lower_leaf(mem, &mut |_, phys, size, _| {
            let n = size.bytes() as usize;
            mem.read_bytes(phys, &mut b[at..at + n]);
            at += n;
        });
        b.extend_from_slice(&(encoded.len() as u64).to_le_bytes());
        b.extend_from_slice(&encoded);
        Self { bytes: b }
    }

    /// Validates `bytes` as a snapshot for `machine`.
    pub fn from_bytes(bytes: Vec<u8>, machine: Machine) -> Result<Self, FormatError> {
        if bytes.len() < HEADER + STATE {
            return Err(FormatError::Truncated);
        }
        if &bytes[..8] != MAGIC {
            return Err(FormatError::BadMagic);
        }
        let version = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        if version != VERSION {
            return Err(FormatError::Version(version));
        }
        let found = u16::from_le_bytes([bytes[12], bytes[13]]);
        if found != machine as u16 {
            return Err(FormatError::Machine {
                expected: machine as u16,
                found,
            });
        }
        let snapshot = Self { bytes };
        let pages = snapshot.page_count();
        // Counts are 32-bit: these products cannot overflow a 64-bit usize.
        let records = HEADER + STATE + snapshot.thread_count() * THREAD;
        let data_start = records + pages * RECORD;
        if snapshot.bytes.len() < data_start {
            return Err(FormatError::Truncated);
        }
        snapshot.check_threads()?;
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
        if len < files_at + 8 || len - files_at - 8 != u64_at(&snapshot.bytes, files_at as usize) {
            return Err(FormatError::Truncated);
        }
        Fs::decode(snapshot.files_bytes()).map_err(|_| FormatError::BadFiles)?;
        Ok(snapshot)
    }

    /// The encoded bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Takes the encoded bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
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

    /// Where the page records start.
    fn records(&self) -> usize {
        HEADER + STATE + self.thread_count() * THREAD
    }

    /// Validates the threads (`from_bytes`; the size was checked).
    fn check_threads(&self) -> Result<(), FormatError> {
        let next_tid = u64_at(&self.bytes, HEADER + 24);
        if self.thread_count() == 0 {
            return Err(FormatError::BadThread { index: 0 });
        }
        let mut tids = Vec::with_capacity(self.thread_count());
        for index in 0..self.thread_count() {
            let w = |i: usize| u64_at(&self.bytes, HEADER + STATE + index * THREAD + i * 8);
            let (tid, kind, addr, bitset, time) = (w(0), w(2), w(3), w(4), w(5));
            let wait_ok = match kind {
                0 => addr == 0 && bitset == 0 && time == NO_DEADLINE,
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

    /// The encoded file system (after the page data and its length).
    pub fn files_bytes(&self) -> &[u8] {
        let pages = self.page_count();
        let data_start = self.records() + pages * RECORD;
        // Pages are stored in record order: the data ends after the last.
        let data_len = pages.checked_sub(1).map_or(0, |last| {
            let page = self.page(last);
            u64_at(&self.bytes, self.records() + last * RECORD + 8) + page.size.bytes()
        });
        &self.bytes[data_start + data_len as usize + 8..]
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
        let start = self.records() + self.page_count() * RECORD + u64_at(b, rec + 8) as usize;
        Page {
            virt: u64_at(b, rec),
            size,
            flags: decode_flags(b[rec + 17]).unwrap_or(PageFlags::USER),
            data: &b[start..start + size.bytes() as usize],
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

    /// Maps page `index` into `pt` with a fresh frame holding its contents.
    pub fn restore_page<F: EntryFormat>(
        &self,
        index: usize,
        pt: &PageTable<F>,
        mem: &mut impl PhysicalMemory,
        frames: &mut impl Frames,
    ) -> Result<(), MapError> {
        let page = self.page(index);
        let phys = match page.size {
            PageSize::Large => frames.alloc_large(),
            _ => frames.alloc(),
        }
        .ok_or(MapError::OutOfFrames)?;
        mem.write_bytes(phys, page.data);
        pt.map(
            mem,
            &mut || frames.alloc(),
            page.virt,
            phys,
            page.size,
            page.flags,
        )
        .inspect_err(|_| frames.free(phys, page.size))
    }

    /// A cell `id` with root `root` (an empty user half, sharing the kernel
    /// half as the architecture needs) in the snapshot's state, its threads'
    /// deadlines counted from monotonic time `now`.
    pub fn cell(&self, id: u32, root: u64, now: u64) -> Cell {
        let s = self.state();
        Cell {
            id,
            root,
            threads: self.threads(now),
            brk_start: s.brk_start,
            brk: s.brk,
            mmap_next: s.mmap_next,
            exit_status: None,
        }
    }
}

#[cfg(test)]
mod tests;
