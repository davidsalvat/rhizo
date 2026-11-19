//! Static 64-bit ELF executables: validation, loading into a cell's address
//! space, and the initial stack (System V ABI / Linux layout).

use crate::memory::{self, write_bytes};
use crate::{STACK_PAGES, STACK_TOP};
use rhizo_hal::PhysicalMemory;
use rhizo_mm::paging::{EntryFormat, MapError, PageFlags, PageSize, PageTable};
use rhizo_mm::{FRAME_SIZE, align_down, align_up};

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
}

const MAX_SEGMENTS: usize = 8;
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_PHDR: u32 = 6;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
/// Highest user address + 1 (lower half of a 48-bit address space).
const USER_END: u64 = 0x0000_8000_0000_0000;

/// One loadable segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub vaddr: u64,
    pub mem_size: u64,
    pub file_offset: u64,
    pub file_size: u64,
    pub flags: PageFlags,
}

/// A validated executable.
#[derive(Debug, Clone, Copy)]
pub struct Image<'a> {
    data: &'a [u8],
    /// Entry point.
    pub entry: u64,
    segments: [Option<Segment>; MAX_SEGMENTS],
    /// Address of the program headers in the loaded image (for `AT_PHDR`).
    pub phdr_addr: u64,
    pub phnum: u64,
    pub phentsize: u64,
}

fn u16_at(d: &[u8], o: usize) -> Result<u16, ElfError> {
    let b = d.get(o..o + 2).ok_or(ElfError::Truncated)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(d: &[u8], o: usize) -> Result<u32, ElfError> {
    let b = d.get(o..o + 4).ok_or(ElfError::Truncated)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u64_at(d: &[u8], o: usize) -> Result<u64, ElfError> {
    let b = d.get(o..o + 8).ok_or(ElfError::Truncated)?;
    Ok(u64::from_le_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
}

impl<'a> Image<'a> {
    /// Validates `data` as a static executable for `machine`.
    pub fn parse(data: &'a [u8], machine: Machine) -> Result<Self, ElfError> {
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
        if kind != 2 {
            return Err(ElfError::NotStaticExecutable(kind));
        }
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
        for i in 0..phnum {
            let ph = (phoff + i * phentsize) as usize;
            let p_type = u32_at(data, ph)?;
            let p_flags = u32_at(data, ph + 4)?;
            let file_offset = u64_at(data, ph + 8)?;
            let vaddr = u64_at(data, ph + 16)?;
            let file_size = u64_at(data, ph + 32)?;
            let mem_size = u64_at(data, ph + 40)?;
            match p_type {
                PT_INTERP | PT_DYNAMIC => return Err(ElfError::DynamicallyLinked),
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
                    if end > USER_END || vaddr < FRAME_SIZE || file_size > mem_size {
                        return Err(ElfError::BadSegment { vaddr });
                    }
                    if file_end > data.len() as u64 {
                        return Err(ElfError::Truncated);
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
            phdr_addr: phdr_addr.or(first_load.map(|v| v + phoff)).unwrap_or(0),
            phnum,
            phentsize,
        })
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

    /// Maps every segment into `pt` with fresh zeroed frames and copies the
    /// file contents (the rest of each segment, its `.bss`, stays zero).
    pub fn load<F: EntryFormat>(
        &self,
        pt: &PageTable<F>,
        mem: &mut impl PhysicalMemory,
        alloc: &mut impl FnMut() -> Option<u64>,
    ) -> Result<(), ElfError> {
        for s in self.segments() {
            let first = align_down(s.vaddr, FRAME_SIZE);
            for page in
                (first..align_up(s.vaddr + s.mem_size, FRAME_SIZE)).step_by(FRAME_SIZE as usize)
            {
                let frame = alloc().ok_or(ElfError::OutOfMemory)?;
                mem.zero_frame(frame);
                // Bytes of the file that fall in this page.
                let lo = page.max(s.vaddr);
                let hi = (page + FRAME_SIZE).min(s.vaddr + s.file_size);
                if hi > lo {
                    let from = (s.file_offset + (lo - s.vaddr)) as usize;
                    write_bytes(
                        mem,
                        frame + (lo - page),
                        &self.data[from..from + (hi - lo) as usize],
                    );
                }
                pt.map(mem, alloc, page, frame, PageSize::Small, s.flags)
                    .map_err(|e| match e {
                        MapError::OutOfFrames => ElfError::OutOfMemory,
                        _ => ElfError::BadSegment { vaddr: s.vaddr },
                    })?;
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
    pub const AT_ENTRY: u64 = 9;
    pub const AT_RANDOM: u64 = 25;
}

/// Maps the stack below [`STACK_TOP`] (leaving the page under it unmapped as a
/// guard) and writes the initial stack: `argc`, `argv`, `envp`, the auxiliary
/// vector, the strings and 16 random bytes. Returns the initial stack pointer
/// (16-byte aligned, pointing at `argc`).
pub fn build_stack<F: EntryFormat>(
    pt: &PageTable<F>,
    mem: &mut impl PhysicalMemory,
    alloc: &mut impl FnMut() -> Option<u64>,
    image: &Image<'_>,
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

    let aux = [
        (auxv::AT_PHDR, image.phdr_addr),
        (auxv::AT_PHENT, image.phentsize),
        (auxv::AT_PHNUM, image.phnum),
        (auxv::AT_PAGESZ, FRAME_SIZE),
        (auxv::AT_ENTRY, image.entry),
        (auxv::AT_RANDOM, random_addr),
        (auxv::AT_NULL, 0),
    ];
    // argc + argv + NULL + envp + NULL + auxv pairs, as 8-byte words.
    let words = 1 + argv.len() + 1 + envp.len() + 1 + aux.len() * 2;
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
    use rhizo_hal::testing::FakePhysicalMemory;
    use rhizo_mm::paging::Aarch64;

    /// Builds a static ELF with the given `(vaddr, flags, contents, mem_size)`
    /// segments, each placed at file offset `0x1000 * (i + 1)`.
    pub(crate) fn elf(machine: u16, kind: u16, segments: &[(u64, u32, &[u8], u64)]) -> Vec<u8> {
        let mut e = vec![0u8; 0x1000 * (segments.len() + 1)];
        e[..6].copy_from_slice(b"\x7fELF\x02\x01");
        e[16..18].copy_from_slice(&kind.to_le_bytes());
        e[18..20].copy_from_slice(&machine.to_le_bytes());
        e[24..32].copy_from_slice(&segments[0].0.to_le_bytes());
        e[32..40].copy_from_slice(&64u64.to_le_bytes());
        e[54..56].copy_from_slice(&56u16.to_le_bytes());
        e[56..58].copy_from_slice(&(segments.len() as u16).to_le_bytes());
        for (i, &(vaddr, flags, data, mem_size)) in segments.iter().enumerate() {
            let ph = 64 + i * 56;
            let off = 0x1000 * (i + 1);
            e[ph..ph + 4].copy_from_slice(&PT_LOAD.to_le_bytes());
            e[ph + 4..ph + 8].copy_from_slice(&flags.to_le_bytes());
            e[ph + 8..ph + 16].copy_from_slice(&(off as u64).to_le_bytes());
            e[ph + 16..ph + 24].copy_from_slice(&vaddr.to_le_bytes());
            e[ph + 32..ph + 40].copy_from_slice(&(data.len() as u64).to_le_bytes());
            e[ph + 40..ph + 48].copy_from_slice(&mem_size.to_le_bytes());
            e.resize(e.len().max(off + data.len()), 0);
            e[off..off + data.len()].copy_from_slice(data);
        }
        e
    }

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

    #[test]
    fn loads_segments_with_permissions_and_zeroed_bss() {
        let data = sample();
        let image = Image::parse(&data, Machine::Aarch64).unwrap();
        assert_eq!(image.entry, 0x40_0000);
        assert_eq!(image.end(), 0x41_2000);
        let (pt, mut mem, mut alloc) = table();
        image.load(&pt, &mut mem, &mut alloc).unwrap();

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
            &image,
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
