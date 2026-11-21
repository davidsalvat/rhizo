//! 64-bit ELF executables: validation, loading into a cell's address
//! space (static executables copied from their image, any executable or
//! interpreter mapped from a file's pages), and the initial stack (System V
//! ABI / Linux layout).

use crate::memory::{self, write_bytes};
use crate::space::{self, Frames};
use crate::{STACK_PAGES, STACK_TOP, USER_END};
use rhizo_hal::{PhysicalMemory, Tlb};
use rhizo_mm::paging::{EntryFormat, MapError, PageFlags, PageSize, PageTable};
use rhizo_mm::{FRAME_SIZE, LARGE_PAGE_SIZE, align_down, align_up};

/// ELF machine numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Machine {
    /// `EM_AARCH64`.
    Aarch64 = 183,
    /// `EM_X86_64`.
    X86_64 = 62,
}

/// Why an image cannot become a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    /// Not an ELF file, or not 64-bit little endian.
    NotElf,
    /// Built for another architecture.
    WrongMachine { expected: u16, found: u16 },
    /// Not an executable (`ET_EXEC`): libraries and PIE are not supported yet.
    NotStaticExecutable(u16),
    /// Has an interpreter or dynamic section.
    DynamicallyLinked,
    /// A segment is both writable and executable (W^X).
    WritableAndExecutable { vaddr: u64 },
    /// A segment lies outside the user half, overlaps another, or is malformed.
    BadSegment { vaddr: u64 },
    /// Headers point outside the file.
    Truncated,
    /// More loadable segments than supported.
    TooManySegments,
    /// No memory for the image or a page table.
    OutOfMemory,
    /// The program's interpreter (`PT_INTERP`) is not in the cell's files,
    /// or is not an interpreter that can be loaded.
    BadInterpreter,
    /// The first instruction (the executable's or its interpreter's entry
    /// point) is past the cell's addresses: entering the cell there would
    /// fault in the kernel on x86_64 (finding S18).
    BadEntry(u64),
}

const MAX_SEGMENTS: usize = 8;
/// Largest segment alignment a bias honors (larger ones get this).
const LARGE_ALIGN: u64 = rhizo_mm::LARGE_PAGE_SIZE;
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_PHDR: u32 = 6;
const PF_X: u32 = 1;
const PF_W: u32 = 2;

/// One loadable segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub vaddr: u64,
    pub mem_size: u64,
    pub file_offset: u64,
    pub file_size: u64,
    pub flags: PageFlags,
}

/// A validated executable. Addresses are the file's own: a
/// position-independent one ([`relocatable`](Self::relocatable)) is loaded
/// at a bias added to each of them.
#[derive(Debug, Clone, Copy)]
pub struct Image<'a> {
    /// The file, or its first bytes (headers and interpreter path).
    data: &'a [u8],
    /// Entry point.
    pub entry: u64,
    segments: [Option<Segment>; MAX_SEGMENTS],
    /// Address of the program headers in the loaded image (for `AT_PHDR`).
    pub phdr_addr: u64,
    pub phnum: u64,
    pub phentsize: u64,
    /// `ET_DYN`: a shared object or position-independent executable.
    pub relocatable: bool,
    /// Largest alignment a segment asks for (a bias is a multiple of it).
    pub align: u64,
    /// Where the interpreter's path (`PT_INTERP`) lies in `data`.
    interp: Option<(usize, usize)>,
}

fn u16_at(d: &[u8], o: usize) -> Result<u16, ElfError> {
    let b = d.get(o..o.saturating_add(2)).ok_or(ElfError::Truncated)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(d: &[u8], o: usize) -> Result<u32, ElfError> {
    let b = d.get(o..o.saturating_add(4)).ok_or(ElfError::Truncated)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u64_at(d: &[u8], o: usize) -> Result<u64, ElfError> {
    let b = d.get(o..o.saturating_add(8)).ok_or(ElfError::Truncated)?;
    Ok(u64::from_le_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
}

impl<'a> Image<'a> {
    /// Validates `data` as a static executable for `machine`.
    pub fn parse(data: &'a [u8], machine: Machine) -> Result<Self, ElfError> {
        Self::parse_with(data, data.len() as u64, machine, false)
    }

    /// Validates a file of `size` bytes starting with `head` as an
    /// executable or shared object for `machine`, statically or dynamically
    /// linked (spec `cells`, dynamically linked executables). Its program
    /// headers and interpreter path must lie in `head`; its segments only in
    /// the file.
    pub fn parse_file(head: &'a [u8], size: u64, machine: Machine) -> Result<Self, ElfError> {
        Self::parse_with(head, size, machine, true)
    }

    fn parse_with(
        data: &'a [u8],
        size: u64,
        machine: Machine,
        any: bool,
    ) -> Result<Self, ElfError> {
        const ET_EXEC: u16 = 2;
        const ET_DYN: u16 = 3;
        if data.get(..6) != Some(b"\x7fELF\x02\x01") {
            return Err(ElfError::NotElf);
        }
        let found = u16_at(data, 18)?;
        if found != machine as u16 {
            return Err(ElfError::WrongMachine {
                expected: machine as u16,
                found,
            });
        }
        let kind = u16_at(data, 16)?;
        if kind != ET_EXEC && !(any && kind == ET_DYN) {
            return Err(ElfError::NotStaticExecutable(kind));
        }
        let relocatable = kind == ET_DYN;
        let entry = u64_at(data, 24)?;
        let phoff = u64_at(data, 32)?;
        let phentsize = u16_at(data, 54)? as u64;
        let phnum = u16_at(data, 56)? as u64;
        if phentsize < 56 {
            return Err(ElfError::Truncated);
        }

        let mut segments = [None; MAX_SEGMENTS];
        let mut count = 0;
        let mut phdr_addr = None;
        let mut first_load = None;
        let mut interp = None;
        let mut align = FRAME_SIZE;
        for i in 0..phnum {
            let ph = i
                .checked_mul(phentsize)
                .and_then(|at| at.checked_add(phoff))
                .ok_or(ElfError::Truncated)? as usize;
            let p_type = u32_at(data, ph)?;
            let p_flags = u32_at(data, ph + 4)?;
            let file_offset = u64_at(data, ph + 8)?;
            let vaddr = u64_at(data, ph + 16)?;
            let file_size = u64_at(data, ph + 32)?;
            let mem_size = u64_at(data, ph + 40)?;
            match p_type {
                PT_INTERP | PT_DYNAMIC if !any => return Err(ElfError::DynamicallyLinked),
                PT_INTERP => {
                    // A path ending in NUL, wholly in `data`.
                    let path = usize::try_from(file_offset)
                        .ok()
                        .zip(usize::try_from(file_size).ok())
                        .and_then(|(at, len)| Some((at, data.get(at..at.checked_add(len)?)?)))
                        .ok_or(ElfError::Truncated)?;
                    let len = path.1.iter().position(|&b| b == 0);
                    interp = Some((path.0, len.ok_or(ElfError::BadInterpreter)?));
                }
                PT_PHDR => phdr_addr = Some(vaddr),
                PT_LOAD => {
                    if p_flags & PF_W != 0 && p_flags & PF_X != 0 {
                        return Err(ElfError::WritableAndExecutable { vaddr });
                    }
                    let end = vaddr
                        .checked_add(mem_size)
                        .ok_or(ElfError::BadSegment { vaddr })?;
                    let file_end = file_offset
                        .checked_add(file_size)
                        .ok_or(ElfError::Truncated)?;
                    // A relocatable image's addresses are checked once its
                    // bias is known (`map_file`).
                    let low = !relocatable && vaddr < FRAME_SIZE;
                    if end > USER_END || low || file_size > mem_size {
                        return Err(ElfError::BadSegment { vaddr });
                    }
                    // Pages map file pages: same offset within a page.
                    if any && vaddr % FRAME_SIZE != file_offset % FRAME_SIZE {
                        return Err(ElfError::BadSegment { vaddr });
                    }
                    if file_end > size {
                        return Err(ElfError::Truncated);
                    }
                    let p_align = u64_at(data, ph + 48)?;
                    if p_align.is_power_of_two() && p_align <= LARGE_ALIGN {
                        align = align.max(p_align);
                    }
                    let mut flags = PageFlags::USER;
                    if p_flags & PF_W != 0 {
                        flags = flags | PageFlags::WRITABLE;
                    }
                    if p_flags & PF_X == 0 {
                        flags = flags | PageFlags::NO_EXECUTE;
                    }
                    let segment = Segment {
                        vaddr,
                        mem_size,
                        file_offset,
                        file_size,
                        flags,
                    };
                    // Segments must not share pages: permissions are per page.
                    for s in segments.iter().flatten() {
                        let s: &Segment = s;
                        let (a0, a1) = (
                            align_down(s.vaddr, FRAME_SIZE),
                            align_up(s.vaddr + s.mem_size, FRAME_SIZE),
                        );
                        let (b0, b1) = (align_down(vaddr, FRAME_SIZE), align_up(end, FRAME_SIZE));
                        if a0 < b1 && b0 < a1 {
                            return Err(ElfError::BadSegment { vaddr });
                        }
                    }
                    if count == MAX_SEGMENTS {
                        return Err(ElfError::TooManySegments);
                    }
                    if file_offset == 0 {
                        first_load = Some(vaddr);
                    }
                    segments[count] = Some(segment);
                    count += 1;
                }
                _ => {}
            }
        }
        if count == 0 {
            return Err(ElfError::BadSegment { vaddr: 0 });
        }
        Ok(Self {
            data,
            entry,
            segments,
            phdr_addr: phdr_addr
                .or(first_load.map(|v| v.wrapping_add(phoff)))
                .unwrap_or(0),
            phnum,
            phentsize,
            relocatable,
            align,
            interp,
        })
    }

    /// The path of the program's interpreter (`PT_INTERP`), if it has one.
    pub fn interpreter(&self) -> Option<&'a [u8]> {
        let (at, len) = self.interp?;
        self.data.get(at..at + len)
    }

    /// The loadable segments.
    pub fn segments(&self) -> impl Iterator<Item = &Segment> {
        self.segments.iter().flatten()
    }

    /// End of the highest segment, page aligned: where the heap starts.
    pub fn end(&self) -> u64 {
        self.segments()
            .map(|s| align_up(s.vaddr + s.mem_size, FRAME_SIZE))
            .max()
            .unwrap_or(0)
    }

    /// The addresses the loaded segments lie in: from the first page of
    /// the lowest to [`end`](Self::end).
    pub fn span(&self) -> core::ops::Range<u64> {
        let start = self
            .segments()
            .map(|s| align_down(s.vaddr, FRAME_SIZE))
            .min()
            .unwrap_or(0);
        start..self.end()
    }

    /// Maps every segment into `pt` with fresh zeroed frames and copies the
    /// file contents (the rest of each segment, its `.bss`, stays zero).
    /// Each 2 MiB-aligned 2 MiB piece that lies wholly in one segment gets a
    /// 2 MiB page when a 2 MiB frame is free (design D1 of
    /// `add-large-image-pages`); the rest gets 4 KiB pages.
    pub fn load<F: EntryFormat>(
        &self,
        pt: &PageTable<F>,
        mem: &mut impl PhysicalMemory,
        frames: &mut impl Frames,
    ) -> Result<(), ElfError> {
        for s in self.segments() {
            let end = align_up(s.vaddr + s.mem_size, FRAME_SIZE);
            let mut page = align_down(s.vaddr, FRAME_SIZE);
            while page < end {
                let large = if page.is_multiple_of(LARGE_PAGE_SIZE) && end - page >= LARGE_PAGE_SIZE
                {
                    frames.alloc_large()
                } else {
                    None
                };
                let (frame, size) = match large {
                    Some(frame) => (frame, PageSize::Large),
                    None => (
                        frames.alloc().ok_or(ElfError::OutOfMemory)?,
                        PageSize::Small,
                    ),
                };
                for f in (frame..frame + size.bytes()).step_by(FRAME_SIZE as usize) {
                    mem.zero_frame(f);
                }
                // Bytes of the file that fall in this page.
                let lo = page.max(s.vaddr);
                let hi = (page + size.bytes()).min(s.vaddr + s.file_size);
                if hi > lo {
                    let from = (s.file_offset + (lo - s.vaddr)) as usize;
                    write_bytes(
                        mem,
                        frame + (lo - page),
                        &self.data[from..from + (hi - lo) as usize],
                    );
                }
                if let Err(e) = pt.map(mem, &mut || frames.alloc(), page, frame, size, s.flags) {
                    frames.free(frame, size);
                    return Err(match e {
                        MapError::OutOfFrames => ElfError::OutOfMemory,
                        _ => ElfError::BadSegment { vaddr: s.vaddr },
                    });
                }
                page += size.bytes();
            }
        }
        Ok(())
    }

    /// Maps every segment, moved by `bias` (0 unless
    /// [`relocatable`](Self::relocatable)), from the file's pages as Linux
    /// does (design D2 of `add-dynamic-linking`): a page of the segment's
    /// file part is the frame `page(i)` gives for page `i` of the file
    /// (shared, copy-on-write where writable); the page where the file part
    /// ends and `.bss` begins is a private copy with the `.bss` zeroed; the
    /// pages of `.bss` alone are zeroed. `code(phys)` is called for each
    /// private executable page written here. On failure the caller destroys
    /// `pt`, which returns every frame.
    #[allow(clippy::too_many_arguments)]
    pub fn map_file<F: EntryFormat, M: PhysicalMemory, A: Frames>(
        &self,
        bias: u64,
        pt: &PageTable<F>,
        mem: &mut M,
        tlb: &mut impl Tlb,
        frames: &mut A,
        page: &mut impl FnMut(u64, &mut M, &mut A) -> Option<u64>,
        code: &mut impl FnMut(u64),
    ) -> Result<(), ElfError> {
        let span = self.span();
        let fits = span
            .start
            .checked_add(bias)
            .zip(span.end.checked_add(bias))
            .is_some_and(|(lo, hi)| lo >= FRAME_SIZE && hi <= USER_END);
        let moved = bias != 0 && (!self.relocatable || !bias.is_multiple_of(FRAME_SIZE));
        if !fits || moved {
            return Err(ElfError::BadSegment { vaddr: span.start });
        }
        let error = |vaddr| {
            move |e| match e {
                MapError::OutOfFrames => ElfError::OutOfMemory,
                _ => ElfError::BadSegment { vaddr },
            }
        };
        for s in self.segments() {
            let start = bias + s.vaddr;
            let first = align_down(start, FRAME_SIZE);
            let file_end = start + s.file_size;
            let end = align_up(start + s.mem_size, FRAME_SIZE);
            // Page `first` holds file page `first_page`.
            let first_page = s
                .file_offset
                .checked_sub(start - first)
                .ok_or(ElfError::BadSegment { vaddr: s.vaddr })?
                / FRAME_SIZE;
            // Whole file pages: up to the page where `.bss` begins, or to
            // the end of the last page when there is no `.bss`.
            let shared_end = if s.file_size == 0 {
                first
            } else if s.mem_size > s.file_size {
                align_down(file_end, FRAME_SIZE)
            } else {
                align_up(file_end, FRAME_SIZE)
            };
            if shared_end > first {
                space::map_frames(
                    pt,
                    mem,
                    tlb,
                    frames,
                    first,
                    shared_end - first,
                    s.flags,
                    &mut |i, mem, frames| page(first_page + i, mem, frames),
                )
                .map_err(error(s.vaddr))?;
            }
            let mut zero_from = shared_end;
            if zero_from < end && !file_end.is_multiple_of(FRAME_SIZE) && s.file_size > 0 {
                // The page shared by the file's last bytes and `.bss`.
                let index = first_page + (zero_from - first) / FRAME_SIZE;
                let source = page(index, mem, frames).ok_or(ElfError::OutOfMemory)?;
                let copy = frames.alloc();
                if let Some(copy) = copy {
                    let mut bytes = [0u8; FRAME_SIZE as usize];
                    let keep = (file_end - zero_from) as usize;
                    mem.read_bytes(source, &mut bytes[..keep]);
                    mem.zero_frame(copy);
                    write_bytes(mem, copy, &bytes[..keep]);
                }
                frames.free(source, PageSize::Small);
                let copy = copy.ok_or(ElfError::OutOfMemory)?;
                if let Err(e) = space::map_frames(
                    pt,
                    mem,
                    tlb,
                    frames,
                    zero_from,
                    FRAME_SIZE,
                    s.flags,
                    &mut |_, _, _| Some(copy),
                ) {
                    frames.free(copy, PageSize::Small);
                    return Err(error(s.vaddr)(e));
                }
                if s.flags.is_executable() {
                    code(copy);
                }
                zero_from += FRAME_SIZE;
            }
            if zero_from < end {
                space::map_anonymous(pt, mem, tlb, frames, zero_from, end - zero_from, s.flags)
                    .map_err(error(s.vaddr))?;
            }
        }
        Ok(())
    }
}

/// Auxiliary-vector keys used by Rhizo.
mod auxv {
    pub const AT_NULL: u64 = 0;
    pub const AT_PHDR: u64 = 3;
    pub const AT_PHENT: u64 = 4;
    pub const AT_PHNUM: u64 = 5;
    pub const AT_PAGESZ: u64 = 6;
    pub const AT_BASE: u64 = 7;
    pub const AT_ENTRY: u64 = 9;
    pub const AT_RANDOM: u64 = 25;
}

/// What the auxiliary vector tells a program about its executable, at the
/// addresses it was loaded to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aux {
    /// The executable's program headers (`AT_PHDR`, `AT_PHENT`, `AT_PHNUM`).
    pub phdr: u64,
    pub phent: u64,
    pub phnum: u64,
    /// The executable's entry point (`AT_ENTRY`).
    pub entry: u64,
    /// Where the interpreter was loaded (`AT_BASE`), if there is one.
    pub base: Option<u64>,
}

impl Aux {
    /// The values for `image` loaded at `bias`, without an interpreter.
    pub fn of(image: &Image<'_>, bias: u64) -> Self {
        Self {
            phdr: image.phdr_addr.wrapping_add(bias),
            phent: image.phentsize,
            phnum: image.phnum,
            entry: image.entry.wrapping_add(bias),
            base: None,
        }
    }
}

/// Maps the stack below [`STACK_TOP`] (leaving the page under it unmapped as a
/// guard) and writes the initial stack: `argc`, `argv`, `envp`, the auxiliary
/// vector (`aux`), the strings and 16 random bytes. Returns the initial
/// stack pointer (16-byte aligned, pointing at `argc`).
pub fn build_stack<F: EntryFormat>(
    pt: &PageTable<F>,
    mem: &mut impl PhysicalMemory,
    alloc: &mut impl FnMut() -> Option<u64>,
    aux: &Aux,
    argv: &[&[u8]],
    envp: &[&[u8]],
    random: [u8; 16],
) -> Result<u64, ElfError> {
    let rw = PageFlags::USER | PageFlags::WRITABLE | PageFlags::NO_EXECUTE;
    for i in 1..=STACK_PAGES {
        let frame = alloc().ok_or(ElfError::OutOfMemory)?;
        mem.zero_frame(frame);
        pt.map(
            mem,
            alloc,
            STACK_TOP - i * FRAME_SIZE,
            frame,
            PageSize::Small,
            rw,
        )
        .map_err(|_| ElfError::OutOfMemory)?;
    }

    let mut top = STACK_TOP;
    let mut push_bytes = |mem: &mut _, bytes: &[u8]| -> Result<u64, ElfError> {
        top -= bytes.len() as u64;
        memory::copy_to(pt, mem, top, bytes).map_err(|_| ElfError::OutOfMemory)?;
        Ok(top)
    };
    let random_addr = push_bytes(mem, &random)?;
    let mut string_addrs = [0u64; 64];
    let count = argv.len() + envp.len();
    if count > string_addrs.len() {
        return Err(ElfError::OutOfMemory);
    }
    for (i, s) in argv.iter().chain(envp.iter()).enumerate() {
        push_bytes(mem, &[0])?;
        string_addrs[i] = push_bytes(mem, s)?;
    }

    let all = [
        (auxv::AT_PHDR, aux.phdr),
        (auxv::AT_PHENT, aux.phent),
        (auxv::AT_PHNUM, aux.phnum),
        (auxv::AT_PAGESZ, FRAME_SIZE),
        (auxv::AT_BASE, aux.base.unwrap_or(0)),
        (auxv::AT_ENTRY, aux.entry),
        (auxv::AT_RANDOM, random_addr),
        (auxv::AT_NULL, 0),
    ];
    // `AT_BASE` only for a program with an interpreter (static ones keep
    // the stack they had).
    let aux = all
        .iter()
        .copied()
        .filter(|&(key, _)| key != auxv::AT_BASE || aux.base.is_some());
    // argc + argv + NULL + envp + NULL + auxv pairs, as 8-byte words.
    let words = 1 + argv.len() + 1 + envp.len() + 1 + aux.clone().count() * 2;
    let sp = align_down(top - words as u64 * 8, 16);
    let mut at = sp;
    let mut put = |mem: &mut _, value: u64| -> Result<(), ElfError> {
        memory::copy_to(pt, mem, at, &value.to_le_bytes()).map_err(|_| ElfError::OutOfMemory)?;
        at += 8;
        Ok(())
    };
    put(mem, argv.len() as u64)?;
    for &addr in &string_addrs[..argv.len()] {
        put(mem, addr)?;
    }
    put(mem, 0)?;
    for &addr in &string_addrs[argv.len()..count] {
        put(mem, addr)?;
    }
    put(mem, 0)?;
    for (key, value) in aux {
        put(mem, key)?;
        put(mem, value)?;
    }
    Ok(sp)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::testing::CountingFrames;
    use rhizo_hal::testing::FakePhysicalMemory;
    use rhizo_mm::paging::Aarch64;

    pub(crate) use crate::testing::{elf, elf_with};

    const R_X: u32 = 5;
    const R_W: u32 = 6;

    fn sample() -> Vec<u8> {
        elf(
            183,
            2,
            &[
                (0x40_0000, R_X, b"CODE", 4),
                (0x41_0000, R_W, b"DATA", 0x2000),
            ],
        )
    }

    #[test]
    fn rejects_what_is_not_a_static_executable() {
        assert_eq!(
            Image::parse(b"MZ", Machine::Aarch64).unwrap_err(),
            ElfError::NotElf
        );
        assert_eq!(
            Image::parse(&sample(), Machine::X86_64).unwrap_err(),
            ElfError::WrongMachine {
                expected: 62,
                found: 183
            }
        );
        let lib = elf(183, 3, &[(0x40_0000, R_X, b"x", 1)]);
        assert_eq!(
            Image::parse(&lib, Machine::Aarch64).unwrap_err(),
            ElfError::NotStaticExecutable(3)
        );
        let wx = elf(183, 2, &[(0x40_0000, 7, b"x", 1)]);
        assert_eq!(
            Image::parse(&wx, Machine::Aarch64).unwrap_err(),
            ElfError::WritableAndExecutable { vaddr: 0x40_0000 }
        );
        let kernel = elf(183, 2, &[(0xffff_8000_0000_0000, R_X, b"x", 1)]);
        assert!(matches!(
            Image::parse(&kernel, Machine::Aarch64),
            Err(ElfError::BadSegment { .. })
        ));
        let shared = elf(
            183,
            2,
            &[(0x40_0000, R_X, b"x", 1), (0x40_0800, R_W, b"y", 1)],
        );
        assert!(matches!(
            Image::parse(&shared, Machine::Aarch64),
            Err(ElfError::BadSegment { .. })
        ));
        let mut dynamic = sample();
        dynamic[64..68].copy_from_slice(&PT_INTERP.to_le_bytes());
        assert_eq!(
            Image::parse(&dynamic, Machine::Aarch64).unwrap_err(),
            ElfError::DynamicallyLinked
        );
    }

    fn table() -> (
        PageTable<Aarch64>,
        FakePhysicalMemory,
        impl FnMut() -> Option<u64>,
    ) {
        let mut mem = FakePhysicalMemory::new();
        let mut next = 0x100_0000u64;
        let mut alloc = move || {
            next += 0x1000;
            Some(next)
        };
        let pt = PageTable::new(&mut mem, &mut alloc).unwrap();
        (pt, mem, alloc)
    }

    /// A dynamically linked program parses from its first bytes: position
    /// independent, with its interpreter's path; a static parse still
    /// refuses it, and a broken interpreter header is an error.
    #[test]
    fn programs_with_an_interpreter_parse_from_their_headers() {
        let data = elf_with(
            62,
            3,
            &[(0, R_X, b"CODE", 4), (0x1000, R_W, b"D", 0x1800)],
            Some(b"/lib/ld.so"),
        );
        let head = &data[..0x1000];
        let image = Image::parse_file(head, data.len() as u64, Machine::X86_64).unwrap();
        assert!(image.relocatable);
        assert_eq!(image.interpreter(), Some(&b"/lib/ld.so"[..]));
        assert_eq!(image.span(), 0..0x3000);
        assert_eq!(
            Image::parse(&data, Machine::X86_64).unwrap_err(),
            ElfError::NotStaticExecutable(3)
        );
        let exec = elf_with(62, 2, &[(0x40_0000, R_X, b"C", 1)], Some(b"/ld"));
        assert_eq!(
            Image::parse(&exec, Machine::X86_64).unwrap_err(),
            ElfError::DynamicallyLinked
        );
        let image = Image::parse_file(&exec, exec.len() as u64, Machine::X86_64).unwrap();
        assert!(!image.relocatable && image.interpreter() == Some(&b"/ld"[..]));

        // The segments must be in the file, not in the head.
        assert_eq!(
            Image::parse_file(head, 0x1800, Machine::X86_64).unwrap_err(),
            ElfError::Truncated
        );
        let mut unterminated = data.clone();
        unterminated[0x800 + 10] = b'x';
        assert_eq!(
            Image::parse_file(&unterminated, data.len() as u64, Machine::X86_64).unwrap_err(),
            ElfError::BadInterpreter
        );
        assert_eq!(
            Image::parse_file(&data[..0x805], data.len() as u64, Machine::X86_64).unwrap_err(),
            ElfError::Truncated,
            "the path must be in the head"
        );
        // A page of the segment cannot be a page of the file.
        let skewed = elf_with(62, 3, &[(0x10, R_X, b"C", 1)], None);
        assert_eq!(
            Image::parse_file(&skewed, skewed.len() as u64, Machine::X86_64).unwrap_err(),
            ElfError::BadSegment { vaddr: 0x10 }
        );
    }

    /// Code may not end at the last canonical address: a `syscall` there
    /// returns to a non-canonical one (spec `isolation`).
    #[test]
    fn the_last_page_of_the_lower_half_is_refused() {
        let last = elf(62, 2, &[(USER_END, R_X, &[0x0f, 0x05], 0x1000)]);
        assert_eq!(
            Image::parse(&last, Machine::X86_64).unwrap_err(),
            ElfError::BadSegment { vaddr: USER_END }
        );
        let below = elf(62, 2, &[(USER_END - 0x1000, R_X, &[0x0f, 0x05], 0x1000)]);
        assert!(Image::parse(&below, Machine::X86_64).is_ok());
    }

    proptest::proptest! {
        /// Any bytes, however broken their headers, parse to an image or an
        /// error, never a panic.
        #[test]
        fn hostile_headers_never_panic(
            mut bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..400),
            phoff in proptest::prelude::any::<u64>(),
            phnum in proptest::prelude::any::<u16>(),
            phentsize in proptest::prelude::any::<u16>(),
        ) {
            let mut e = sample();
            e.truncate(e.len().min(64 + bytes.len()));
            if e.len() >= 64 {
                e[32..40].copy_from_slice(&phoff.to_le_bytes());
                e[54..56].copy_from_slice(&phentsize.to_le_bytes());
                e[56..58].copy_from_slice(&phnum.to_le_bytes());
            }
            bytes.splice(0..0, e);
            let _ = Image::parse(&bytes, Machine::Aarch64);
            let _ = Image::parse_file(&bytes, phoff, Machine::Aarch64);
        }
    }

    /// Whole 2 MiB-aligned pieces of a segment get 2 MiB pages, the rest
    /// 4 KiB pages, with the file's bytes where they belong in both; with no
    /// 2 MiB frame free, 4 KiB pages only.
    #[test]
    fn whole_2mib_pieces_of_a_segment_get_2mib_pages() {
        let text: Vec<u8> = (0..0x1100u32).map(|i| (i % 251) as u8).collect();
        let data = elf(183, 2, &[(0x3f_f000, R_X, &text, 0x40_2000)]);
        let image = Image::parse(&data, Machine::Aarch64).unwrap();
        for large in [true, false] {
            let mut mem = FakePhysicalMemory::new();
            let mut frames = CountingFrames::new(1024);
            let pt = PageTable::<Aarch64>::new(&mut mem, &mut || frames.alloc()).unwrap();
            image
                .load(&pt, &mut mem, &mut NoLarge(&mut frames, !large))
                .unwrap();
            for at in [
                0x3f_f000, 0x40_0000, 0x40_00ff, 0x40_0100, 0x5f_f000, 0x60_0000, 0x7f_ffff,
                0x80_0000,
            ] {
                let t = pt.translate(&mem, at).unwrap();
                let expected = if large && (0x40_0000..0x80_0000).contains(&at) {
                    PageSize::Large
                } else {
                    PageSize::Small
                };
                assert_eq!(t.size, expected, "{at:#x}");
                let mut byte = [0u8];
                mem.read_bytes(t.phys, &mut byte);
                let i = (at - 0x3f_f000) as usize;
                assert_eq!(byte[0], text.get(i).copied().unwrap_or(0), "{at:#x}");
            }
            crate::space::destroy(pt, &mut mem, &mut frames);
            assert_eq!(frames.outstanding(), 0);
        }
    }

    /// Frames that refuse 2 MiB when told to.
    struct NoLarge<'a>(&'a mut CountingFrames, bool);

    impl Frames for NoLarge<'_> {
        fn alloc(&mut self) -> Option<u64> {
            self.0.alloc()
        }
        fn alloc_large(&mut self) -> Option<u64> {
            if self.1 { None } else { self.0.alloc_large() }
        }
        fn free(&mut self, phys: u64, size: PageSize) {
            self.0.free(phys, size)
        }
        fn share(&mut self, phys: u64) {
            self.0.share(phys)
        }
        fn is_shared(&self, phys: u64) -> bool {
            self.0.is_shared(phys)
        }
    }

    #[test]
    fn loads_segments_with_permissions_and_zeroed_bss() {
        let data = sample();
        let image = Image::parse(&data, Machine::Aarch64).unwrap();
        assert_eq!(image.entry, 0x40_0000);
        assert_eq!(image.end(), 0x41_2000);
        let mut mem = FakePhysicalMemory::new();
        let mut frames = CountingFrames::new(1024);
        let pt = PageTable::<Aarch64>::new(&mut mem, &mut || frames.alloc()).unwrap();
        image.load(&pt, &mut mem, &mut frames).unwrap();

        let mut code = [0u8; 4];
        memory::copy_from(&pt, &mem, 0x40_0000, &mut code).unwrap();
        assert_eq!(&code, b"CODE");
        let t = pt.translate(&mem, 0x40_0000).unwrap();
        assert!(
            t.flags.is_executable() && !t.flags.is_writable() && t.flags.contains(PageFlags::USER)
        );

        let mut tail = [0xffu8; 8];
        memory::copy_from(&pt, &mem, 0x41_1ff8, &mut tail).unwrap();
        assert_eq!(tail, [0; 8], ".bss is zero");
        let t = pt.translate(&mem, 0x41_1000).unwrap();
        assert!(t.flags.is_writable() && !t.flags.is_executable());
    }

    #[test]
    fn stack_has_argc_argv_envp_and_auxv() {
        let data = sample();
        let image = Image::parse(&data, Machine::Aarch64).unwrap();
        let (pt, mut mem, mut alloc) = table();
        let sp = build_stack(
            &pt,
            &mut mem,
            &mut alloc,
            &Aux::of(&image, 0),
            &[b"hello", b"-v"],
            &[b"A=1"],
            [7; 16],
        )
        .unwrap();
        assert_eq!(sp % 16, 0);
        let word = |at: u64| {
            let mut b = [0u8; 8];
            memory::copy_from(&pt, &mem, at, &mut b).unwrap();
            u64::from_le_bytes(b)
        };
        let string = |at: u64| {
            let mut out = Vec::new();
            let mut a = at;
            loop {
                let mut b = [0u8; 1];
                memory::copy_from(&pt, &mem, a, &mut b).unwrap();
                if b[0] == 0 {
                    return out;
                }
                out.push(b[0]);
                a += 1;
            }
        };
        assert_eq!(word(sp), 2, "argc");
        assert_eq!(string(word(sp + 8)), b"hello");
        assert_eq!(string(word(sp + 16)), b"-v");
        assert_eq!(word(sp + 24), 0);
        assert_eq!(string(word(sp + 32)), b"A=1");
        assert_eq!(word(sp + 40), 0);
        let mut aux = sp + 48;
        let mut seen = Vec::new();
        loop {
            let (k, v) = (word(aux), word(aux + 8));
            if k == 0 {
                break;
            }
            seen.push((k, v));
            aux += 16;
        }
        assert!(seen.contains(&(auxv::AT_PAGESZ, 4096)));
        assert!(seen.contains(&(auxv::AT_ENTRY, 0x40_0000)));
        let random = seen.iter().find(|(k, _)| *k == auxv::AT_RANDOM).unwrap().1;
        let mut r = [0u8; 16];
        memory::copy_from(&pt, &mem, random, &mut r).unwrap();
        assert_eq!(r, [7; 16]);
        // The page above the stack top is not mapped (and below is the guard).
        assert!(pt.translate(&mem, STACK_TOP).is_none());
        assert!(
            pt.translate(&mem, STACK_TOP - (STACK_PAGES + 1) * FRAME_SIZE)
                .is_none()
        );
    }
}
