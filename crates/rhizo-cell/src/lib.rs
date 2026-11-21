//! Agent cells: the cell model, its address space and loading of static
//! Linux executables. Everything works through `rhizo-hal` interfaces and
//! `rhizo-mm` page tables, so it is tested on the host.

#![cfg_attr(not(any(test, feature = "testing")), no_std)]

extern crate alloc;

pub mod elf;
pub mod images;
pub mod memory;
pub mod space;
pub mod threads;

use rhizo_hal::{PhysicalMemory, Tlb};
use rhizo_mm::paging::{EntryFormat, PageTable};
use space::Frames;
use threads::{Thread, Threads};

/// End (exclusive) of the addresses a cell may use or name in a system
/// call (spec `isolation`): the lower half without its last page, as on
/// Linux, so no instruction can end at the last canonical address (a
/// `syscall` there would return to a non-canonical one).
pub const USER_END: u64 = 0x0000_7fff_ffff_f000;
/// Top of a cell's stack (exclusive); the stack grows down from here.
pub const STACK_TOP: u64 = USER_END;
/// Stack pages mapped when a cell starts.
pub const STACK_PAGES: u64 = 16;
/// Most the stack grows to below [`STACK_TOP`], a page at a time on first
/// touch (the 8 MiB `RLIMIT_STACK` Linux gives a process).
pub const STACK_LIMIT: u64 = 8 << 20;
/// First address used for anonymous mappings (`mmap`).
pub const MMAP_BASE: u64 = 0x0000_4000_0000_0000;

/// Floating-point and SIMD registers, as plain data: on AArch64 `V0`–`V31`
/// (words 0–63), then `FPCR` and `FPSR`; on x86_64 the 512-byte `FXSAVE`
/// image. 16-byte aligned, as `FXSAVE` requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(16))]
pub struct FpState(pub [u64; 66]);

impl FpState {
    /// The state a program starts with on `machine`: all zero, except the
    /// x87 control word (`0x37f`) and `MXCSR` (`0x1f80`) on x86_64, as Linux.
    pub fn initial(machine: elf::Machine) -> Self {
        let mut words = [0u64; 66];
        if machine == elf::Machine::X86_64 {
            words[0] = 0x37f; // FCW, bytes 0-1
            words[3] = 0x1f80; // MXCSR, bytes 24-27
        }
        Self(words)
    }
}

impl Default for FpState {
    fn default() -> Self {
        Self([0; 66])
    }
}

/// Everything needed to resume a cell's execution, as plain data on both
/// architectures (design decision D2). The AArch64 entry code saves and
/// restores it in place, so its layout is fixed: `pc` at 0, `sp` at 8, `gpr`
/// at 16, `tls` at 264, `flags` at 272, `fp` at 288.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C, align(16))]
pub struct Registers {
    /// Program counter.
    pub pc: u64,
    /// User stack pointer.
    pub sp: u64,
    /// General-purpose registers (x0–x30 on AArch64; on x86_64 the order is
    /// rax, rbx, rcx, rdx, rsi, rdi, rbp, r8–r15).
    pub gpr: [u64; 31],
    /// Thread pointer (`TPIDR_EL0` or the `FS` base).
    pub tls: u64,
    /// Processor flags (`SPSR_EL1` or `RFLAGS`).
    pub flags: u64,
    /// Floating-point and SIMD state.
    pub fp: FpState,
}

/// A cell: plain data, no pointers into kernel memory (spec `cells`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// Identifier, unique while the cell exists.
    pub id: u32,
    /// Physical address of the cell's root page table.
    pub root: u64,
    /// Its threads (spec `threads`).
    pub threads: Threads,
    /// Start of the heap (end of the loaded image, page aligned).
    pub brk_start: u64,
    /// Current program break.
    pub brk: u64,
    /// Next free address for anonymous mappings.
    pub mmap_next: u64,
    /// Exit status, once the cell has exited.
    pub exit_status: Option<i32>,
}

impl Cell {
    /// A new cell rooted at `root`, about to start at `entry` with stack `sp`;
    /// its first thread's identifier is the cell's.
    pub fn new(id: u32, root: u64, entry: u64, sp: u64, brk_start: u64) -> Self {
        let regs = Registers {
            pc: entry,
            sp,
            ..Registers::default()
        };
        Self {
            id,
            root,
            threads: Threads::new(Thread::new(id, regs)),
            brk_start,
            brk: brk_start,
            mmap_next: MMAP_BASE,
            exit_status: None,
        }
    }

    /// The registers of the current thread.
    pub fn regs(&self) -> &Registers {
        &self.threads.current().regs
    }

    /// The registers of the current thread, mutably.
    pub fn regs_mut(&mut self) -> &mut Registers {
        &mut self.threads.current_mut().regs
    }
}

/// Creates cell `id` from the static executable `data`: [`load`] then
/// [`start`]. On failure every frame is returned.
#[allow(clippy::too_many_arguments)]
pub fn spawn<F: EntryFormat>(
    id: u32,
    data: &[u8],
    machine: elf::Machine,
    kernel: Option<&PageTable<F>>,
    mem: &mut impl PhysicalMemory,
    frames: &mut impl Frames,
    argv: &[&[u8]],
    envp: &[&[u8]],
    random: [u8; 16],
) -> Result<Cell, elf::ElfError> {
    let image = elf::Image::parse(data, machine)?;
    let pt = load(&image, mem, frames)?;
    start(
        id, &image, machine, pt, kernel, mem, frames, argv, envp, random,
    )
}

/// The part of a spawn that depends only on the executable (design D1 of
/// `add-image-cache`): a new hierarchy holding `image`'s segments and
/// nothing else (no stack, no kernel half). On failure every frame is
/// returned.
pub fn load<F: EntryFormat>(
    image: &elf::Image<'_>,
    mem: &mut impl PhysicalMemory,
    frames: &mut impl Frames,
) -> Result<PageTable<F>, elf::ElfError> {
    let pt =
        PageTable::<F>::new(mem, &mut || frames.alloc()).map_err(|_| elf::ElfError::OutOfMemory)?;
    match image.load(&pt, mem, frames) {
        Ok(()) => Ok(pt),
        Err(e) => {
            space::destroy(pt, mem, frames);
            Err(e)
        }
    }
}

/// Starts cell `id` on `pt`, a hierarchy that holds `image`'s segments
/// (from [`load`], or a copy of one): shares the kernel half of `kernel`
/// when given (as x86_64 needs), builds the initial stack and the first
/// thread. On failure every frame of `pt` is returned.
#[allow(clippy::too_many_arguments)]
pub fn start<F: EntryFormat>(
    id: u32,
    image: &elf::Image<'_>,
    machine: elf::Machine,
    pt: PageTable<F>,
    kernel: Option<&PageTable<F>>,
    mem: &mut impl PhysicalMemory,
    frames: &mut impl Frames,
    argv: &[&[u8]],
    envp: &[&[u8]],
    random: [u8; 16],
) -> Result<Cell, elf::ElfError> {
    let layout = Layout {
        pc: image.entry,
        aux: elf::Aux::of(image, 0),
        brk_start: image.end(),
        mmap_next: MMAP_BASE,
    };
    start_at(
        id, &layout, machine, pt, kernel, mem, frames, argv, envp, random,
    )
}

/// Where a position-independent executable is loaded (design D3 of
/// `add-dynamic-linking`): low, so its heap grows a long way before the
/// mappings at [`MMAP_BASE`], and a multiple of every segment alignment.
pub const PIE_BASE: u64 = 0x0000_0001_0000_0000;

/// Maps a program from its files' pages into a new hierarchy (spec `cells`,
/// dynamically linked executables): the executable `main` at its own
/// addresses, or at [`PIE_BASE`] if it is position independent, and its
/// interpreter `interp` (position independent) at the first mapping
/// address, aligned as it asks; mappings continue above it. Page `i` of
/// the executable's file is `page(false, i)`, of the interpreter's
/// `page(true, i)` (see [`elf::Image::map_file`], which also says what
/// `code` is for). Returns the hierarchy (its user half only) and where the
/// program starts. On failure every frame is returned.
pub fn map_program<F: EntryFormat, M: PhysicalMemory, A: Frames>(
    main: &elf::Image<'_>,
    interp: Option<&elf::Image<'_>>,
    mem: &mut M,
    tlb: &mut impl Tlb,
    frames: &mut A,
    page: &mut impl FnMut(bool, u64, &mut M, &mut A) -> Option<u64>,
    code: &mut impl FnMut(u64),
) -> Result<(PageTable<F>, Layout), elf::ElfError> {
    use rhizo_mm::{FRAME_SIZE, align_up};
    if interp.is_some_and(|i| !i.relocatable || i.interpreter().is_some()) {
        return Err(elf::ElfError::BadInterpreter);
    }
    let pt =
        PageTable::<F>::new(mem, &mut || frames.alloc()).map_err(|_| elf::ElfError::OutOfMemory)?;
    let bias = if main.relocatable { PIE_BASE } else { 0 };
    let mut layout = Layout {
        pc: main.entry + bias,
        aux: elf::Aux::of(main, bias),
        brk_start: main.end() + bias,
        mmap_next: MMAP_BASE,
    };
    let mut mapped = main.map_file(
        bias,
        &pt,
        mem,
        tlb,
        frames,
        &mut |i, m, f| page(false, i, m, f),
        code,
    );
    if let (Ok(()), Some(interp)) = (mapped, interp) {
        // Its lowest page at the first mapping address.
        let base = align_up(MMAP_BASE, interp.align).wrapping_sub(interp.span().start);
        layout.pc = interp.entry.wrapping_add(base);
        layout.aux.base = Some(base);
        layout.mmap_next = align_up(interp.end().wrapping_add(base), FRAME_SIZE);
        mapped = interp.map_file(
            base,
            &pt,
            mem,
            tlb,
            frames,
            &mut |i, m, f| page(true, i, m, f),
            code,
        );
    }
    match mapped {
        Ok(()) => Ok((pt, layout)),
        Err(e) => {
            space::destroy(pt, mem, frames);
            Err(e)
        }
    }
}

/// Where a program's images were loaded: what its first thread and its
/// initial stack start from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// First instruction: the executable's entry, or its interpreter's.
    pub pc: u64,
    /// The auxiliary vector's account of the executable.
    pub aux: elf::Aux,
    /// Start of the heap: the end of the executable, page aligned.
    pub brk_start: u64,
    /// First address for mappings (above the interpreter, if any).
    pub mmap_next: u64,
}

/// [`start`] for any `layout` of images loaded into `pt`.
#[allow(clippy::too_many_arguments)]
pub fn start_at<F: EntryFormat>(
    id: u32,
    layout: &Layout,
    machine: elf::Machine,
    pt: PageTable<F>,
    kernel: Option<&PageTable<F>>,
    mem: &mut impl PhysicalMemory,
    frames: &mut impl Frames,
    argv: &[&[u8]],
    envp: &[&[u8]],
    random: [u8; 16],
) -> Result<Cell, elf::ElfError> {
    if let Some(kernel) = kernel {
        pt.share_upper_half(mem, kernel);
    }
    let stack = elf::build_stack(
        &pt,
        mem,
        &mut || frames.alloc(),
        &layout.aux,
        argv,
        envp,
        random,
    );
    match stack {
        Ok(sp) => {
            let mut cell = Cell::new(id, pt.root(), layout.pc, sp, layout.brk_start);
            cell.mmap_next = layout.mmap_next;
            cell.regs_mut().fp = FpState::initial(machine);
            Ok(cell)
        }
        Err(e) => {
            space::destroy(pt, mem, frames);
            Err(e)
        }
    }
}

/// Test doubles for host tests.
#[cfg(any(test, feature = "testing"))]
pub mod testing {
    use crate::space::Frames;
    use rhizo_mm::paging::PageSize;
    use rhizo_mm::{FRAME_SIZE, LARGE_PAGE_SIZE};
    use std::collections::BTreeMap;
    use std::vec::Vec;

    /// Frames from an endless address range, recycled after their last
    /// reference is dropped, that tracks what is outstanding and panics on a
    /// bad or double free.
    #[derive(Debug)]
    pub struct CountingFrames {
        next_small: u64,
        next_large: u64,
        free_small: Vec<u64>,
        free_large: Vec<u64>,
        /// Live pages: size and number of references.
        live: BTreeMap<u64, (PageSize, u32)>,
        /// Allocations fail once this many are outstanding.
        pub limit: usize,
        /// Most references one page may have (sharing past it panics, as
        /// the kernel's counts would).
        pub max_refs: u32,
    }

    /// Builds an ELF file of `kind` (2: executable, 3: shared object) with
    /// the given `(vaddr, flags, contents, mem_size)` segments, each placed
    /// at file offset `0x1000 * (i + 1)`; entry at the first segment.
    pub fn elf(machine: u16, kind: u16, segments: &[(u64, u32, &[u8], u64)]) -> Vec<u8> {
        elf_with(machine, kind, segments, None)
    }

    /// [`elf`] with an interpreter path (`PT_INTERP`, stored at 0x800).
    pub fn elf_with(
        machine: u16,
        kind: u16,
        segments: &[(u64, u32, &[u8], u64)],
        interp: Option<&[u8]>,
    ) -> Vec<u8> {
        const PT_LOAD: u32 = 1;
        const PT_INTERP: u32 = 3;
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
        if let Some(path) = interp {
            let ph = 64 + segments.len() * 56;
            e[ph..ph + 4].copy_from_slice(&PT_INTERP.to_le_bytes());
            e[ph + 8..ph + 16].copy_from_slice(&0x800u64.to_le_bytes());
            let len = path.len() as u64 + 1;
            e[ph + 32..ph + 40].copy_from_slice(&len.to_le_bytes());
            e[0x800..0x800 + path.len()].copy_from_slice(path);
            e[56..58].copy_from_slice(&(segments.len() as u16 + 1).to_le_bytes());
        }
        e
    }

    impl CountingFrames {
        /// Small frames start at 16 MiB, large ones at `large_base` MiB.
        pub fn new(large_base_mib: u64) -> Self {
            Self {
                next_small: 16 << 20,
                next_large: large_base_mib << 20,
                free_small: Vec::new(),
                free_large: Vec::new(),
                live: BTreeMap::new(),
                limit: usize::MAX,
                max_refs: u32::MAX,
            }
        }

        /// Pages allocated and not yet freed.
        pub fn outstanding(&self) -> usize {
            self.live.len()
        }

        fn take(&mut self, size: PageSize) -> Option<u64> {
            if self.live.len() >= self.limit {
                return None;
            }
            let phys = match size {
                PageSize::Small => self.free_small.pop().unwrap_or_else(|| {
                    self.next_small += FRAME_SIZE;
                    self.next_small - FRAME_SIZE
                }),
                _ => self.free_large.pop().unwrap_or_else(|| {
                    self.next_large += LARGE_PAGE_SIZE;
                    self.next_large - LARGE_PAGE_SIZE
                }),
            };
            self.live.insert(phys, (size, 1));
            Some(phys)
        }
    }

    impl Frames for CountingFrames {
        fn alloc(&mut self) -> Option<u64> {
            self.take(PageSize::Small)
        }

        fn alloc_large(&mut self) -> Option<u64> {
            self.take(PageSize::Large)
        }

        fn free(&mut self, phys: u64, size: PageSize) {
            let entry = self
                .live
                .get_mut(&phys)
                .unwrap_or_else(|| panic!("bad free of {phys:#x}"));
            assert_eq!(entry.0, size, "bad free of {phys:#x}");
            entry.1 -= 1;
            if entry.1 > 0 {
                return;
            }
            self.live.remove(&phys);
            match size {
                PageSize::Small => self.free_small.push(phys),
                _ => self.free_large.push(phys),
            }
        }

        fn share(&mut self, phys: u64) {
            let refs = &mut self
                .live
                .get_mut(&phys)
                .unwrap_or_else(|| panic!("share of free {phys:#x}"))
                .1;
            assert!(*refs < self.max_refs, "too many references to {phys:#x}");
            *refs += 1;
        }

        fn is_shared(&self, phys: u64) -> bool {
            self.live.get(&phys).is_some_and(|e| e.1 > 1)
        }

        fn try_share(&mut self, phys: u64) -> bool {
            let full = self.live.get(&phys).is_some_and(|e| e.1 >= self.max_refs);
            if !full {
                self.share(phys);
            }
            !full
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn register_layout_matches_the_entry_code() {
        let r = super::Registers::default();
        let base = core::ptr::addr_of!(r) as usize;
        assert_eq!(core::ptr::addr_of!(r.sp) as usize - base, 8);
        assert_eq!(core::ptr::addr_of!(r.gpr) as usize - base, 16);
        assert_eq!(core::ptr::addr_of!(r.tls) as usize - base, 264);
        assert_eq!(core::ptr::addr_of!(r.flags) as usize - base, 272);
        assert_eq!(core::ptr::addr_of!(r.fp) as usize - base, 288);
        assert_eq!(core::mem::align_of::<super::Registers>(), 16);
    }

    use super::*;

    use crate::testing::CountingFrames;
    use rhizo_hal::testing::FakePhysicalMemory;
    use rhizo_mm::paging::{PageFlags, PageSize, X86_64};

    #[test]
    fn spawn_builds_a_runnable_cell_and_failures_leak_nothing() {
        let data = elf::tests::elf(
            62,
            2,
            &[(0x40_0000, 5, b"CODE", 4), (0x40_1000, 6, b"D", 0x1800)],
        );
        let mut mem = FakePhysicalMemory::new();
        let mut frames = CountingFrames::new(1024);
        let kernel = PageTable::<X86_64>::new(&mut mem, &mut || frames.alloc()).unwrap();
        mem.write_u64(kernel.root() + 511 * 8, 0x1234_5003);
        let tables = frames.outstanding();

        let cell = spawn(
            3,
            &data,
            elf::Machine::X86_64,
            Some(&kernel),
            &mut mem,
            &mut frames,
            &[b"a"],
            &[],
            [0; 16],
        )
        .unwrap();
        assert_eq!(
            (cell.id, cell.regs().pc, cell.brk),
            (3, 0x40_0000, 0x40_3000)
        );
        assert!(cell.regs().sp < STACK_TOP && cell.regs().sp.is_multiple_of(16));
        assert_eq!(
            cell.threads.current().tid,
            3,
            "the first thread is the cell's id"
        );
        assert_eq!(
            mem.read_u64(cell.root + 511 * 8),
            0x1234_5003,
            "kernel half shared"
        );
        space::destroy(
            PageTable::<X86_64>::from_root(cell.root),
            &mut mem,
            &mut frames,
        );
        assert_eq!(frames.outstanding(), tables);

        frames.limit = tables + 5; // enough for the tables, not for the stack
        let err = spawn(
            4,
            &data,
            elf::Machine::X86_64,
            Some(&kernel),
            &mut mem,
            &mut frames,
            &[],
            &[],
            [0; 16],
        );
        assert_eq!(err, Err(elf::ElfError::OutOfMemory));
        assert_eq!(frames.outstanding(), tables);
    }

    /// Every page of the user half: address, size, permissions, contents.
    fn pages(mem: &FakePhysicalMemory, root: u64) -> Vec<(u64, PageSize, PageFlags, Vec<u8>)> {
        let mut out = Vec::new();
        PageTable::<X86_64>::from_root(root).for_each_lower_leaf(mem, &mut |v, p, s, f| {
            let mut bytes = vec![0; s.bytes() as usize];
            mem.read_bytes(p, &mut bytes);
            out.push((v, s, f, bytes));
        });
        out
    }

    /// A cell started on a copy of a loaded space (design D2 of
    /// `add-image-cache`) is the cell a direct spawn makes, and cells
    /// started from one loaded space keep their writes to themselves.
    #[test]
    fn a_cell_on_a_loaded_space_is_a_spawned_cell() {
        let data = elf::tests::elf(
            62,
            2,
            &[(0x40_0000, 5, b"CODE", 4), (0x40_1000, 6, b"DATA", 0x1800)],
        );
        let image = elf::Image::parse(&data, elf::Machine::X86_64).unwrap();
        let mut mem = FakePhysicalMemory::new();
        let mut frames = CountingFrames::new(1024);
        let kernel = PageTable::<X86_64>::new(&mut mem, &mut || frames.alloc()).unwrap();
        mem.write_u64(kernel.root() + 511 * 8, 0x1234_5003);
        let tables = frames.outstanding();
        let argv: &[&[u8]] = &[b"a", b"b"];
        let direct = spawn(
            3,
            &data,
            elf::Machine::X86_64,
            Some(&kernel),
            &mut mem,
            &mut frames,
            argv,
            &[b"E=1"],
            [7; 16],
        )
        .unwrap();

        let loaded = load::<X86_64>(&image, &mut mem, &mut frames).unwrap();
        let start_copy = |id, mem: &mut FakePhysicalMemory, frames: &mut CountingFrames| {
            let pt = loaded.clone_lower_half(mem, frames, image.span()).unwrap();
            start(
                id,
                &image,
                elf::Machine::X86_64,
                pt,
                Some(&kernel),
                mem,
                frames,
                argv,
                &[b"E=1"],
                [7; 16],
            )
            .unwrap()
        };
        let a = start_copy(3, &mut mem, &mut frames);
        let b = start_copy(4, &mut mem, &mut frames);
        assert_eq!(
            a,
            Cell {
                root: a.root,
                ..direct.clone()
            },
            "same registers, threads, break"
        );
        assert_eq!(pages(&mem, a.root), pages(&mem, direct.root));
        assert_eq!(
            mem.read_u64(a.root + 511 * 8),
            0x1234_5003,
            "kernel half shared"
        );

        // A write in one cell's data is seen by no other cell of the image.
        let at = |root, virt| {
            PageTable::<X86_64>::from_root(root)
                .translate(&mem, virt)
                .unwrap()
        };
        let (mine, theirs, pristine) = (
            at(a.root, 0x40_1000),
            at(b.root, 0x40_1000),
            at(loaded.root(), 0x40_1000),
        );
        let code = [a.root, b.root, loaded.root()].map(|r| at(r, 0x40_0000).phys);
        assert!(mine.flags.contains(PageFlags::WRITABLE));
        mem.write_u64(mine.phys, 0xbad);
        let data = u64::from_le_bytes(*b"DATA\0\0\0\0");
        assert_eq!(mem.read_u64(theirs.phys), data);
        assert_eq!(mem.read_u64(pristine.phys), data);
        assert!(code.iter().all(|&c| c == code[0]), "code is shared");

        for root in [direct.root, a.root, b.root, loaded.root()] {
            space::destroy(PageTable::<X86_64>::from_root(root), &mut mem, &mut frames);
        }
        assert_eq!(frames.outstanding(), tables, "every frame returned");
    }

    /// A dynamically linked program mapped from its files' pages (designs
    /// D2 and D3 of `add-dynamic-linking`): the executable at
    /// [`PIE_BASE`], its interpreter at the first mapping address, whole
    /// file pages shared (copy-on-write where writable), the page where
    /// `.bss` starts private and cleared; started at the interpreter with
    /// `AT_BASE`. Every frame comes back, also when memory runs out.
    #[test]
    fn a_program_and_its_interpreter_map_from_file_pages() {
        use rhizo_hal::testing::RecordingTlb;
        use std::collections::BTreeMap;
        let mut main = elf::tests::elf_with(
            62,
            3,
            &[
                (0x1000, 5, b"CODE", 4),
                (0x2000, 6, b"DATA", 0x1800),
                (0x5000, 6, b"RELRO", 5),
            ],
            Some(b"/lib/ld.so"),
        );
        main[0x2004..0x2008].copy_from_slice(b"JUNK"); // past the data, in its page
        let ld = elf::tests::elf_with(
            62,
            3,
            &[(0, 5, b"LDSO", 4), (0x1000, 6, b"L", 0x3000)],
            None,
        );
        let main_image =
            elf::Image::parse_file(&main, main.len() as u64, elf::Machine::X86_64).unwrap();
        let ld_image = elf::Image::parse_file(&ld, ld.len() as u64, elf::Machine::X86_64).unwrap();
        let mut mem = FakePhysicalMemory::new();
        let mut frames = CountingFrames::new(1024);
        let mut tlb = RecordingTlb::default();
        // A page cache: one frame per file page, a reference per mapping.
        let cache = std::cell::RefCell::new(BTreeMap::new());
        let files = [&main, &ld];
        let mut page =
            |interp: bool, i: u64, mem: &mut FakePhysicalMemory, frames: &mut CountingFrames| {
                let known = cache.borrow().get(&(interp, i)).copied();
                let phys = match known {
                    Some(phys) => phys,
                    None => {
                        let phys = frames.alloc()?;
                        mem.zero_frame(phys);
                        let data = files[interp as usize];
                        let lo = (i * 0x1000) as usize;
                        let hi = (lo + 0x1000).min(data.len());
                        memory::write_bytes(mem, phys, &data[lo..hi]);
                        cache.borrow_mut().insert((interp, i), phys);
                        phys
                    }
                };
                frames.share(phys);
                Some(phys)
            };
        let (pt, layout) = map_program::<X86_64, _, _>(
            &main_image,
            Some(&ld_image),
            &mut mem,
            &mut tlb,
            &mut frames,
            &mut page,
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(layout.pc, MMAP_BASE, "the interpreter's entry");
        assert_eq!(layout.aux.entry, PIE_BASE + 0x1000);
        assert_eq!(layout.aux.base, Some(MMAP_BASE));
        assert_eq!(layout.brk_start, PIE_BASE + 0x6000);
        assert_eq!(layout.mmap_next, MMAP_BASE + 0x4000);
        let at = |virt| pt.translate(&mem, virt).unwrap();
        let cached = |interp, i| *cache.borrow().get(&(interp, i)).unwrap();
        let code = at(PIE_BASE + 0x1000);
        assert_eq!(code.phys, cached(false, 1));
        assert!(code.flags.is_executable() && !code.flags.is_writable());
        let data = at(PIE_BASE + 0x2000);
        assert_ne!(data.phys, cached(false, 2), "private: its .bss is cleared");
        assert!(data.flags.is_writable());
        assert_eq!(
            mem.read_u64(data.phys),
            u64::from_le_bytes(*b"DATA\0\0\0\0")
        );
        assert_eq!(mem.read_u64(at(PIE_BASE + 0x3000).phys), 0, ".bss");
        let relro = at(PIE_BASE + 0x5000);
        assert_eq!(relro.phys, cached(false, 3), "shared until written");
        assert!(relro.flags.contains(PageFlags::COPY_ON_WRITE) && !relro.flags.is_writable());
        assert_eq!(at(MMAP_BASE).phys, cached(true, 1));
        assert_eq!(mem.read_u64(at(MMAP_BASE + 0x1000).phys), u64::from(b'L'));
        assert!(pt.translate(&mem, MMAP_BASE + 0x4000).is_none());

        let kernel = PageTable::<X86_64>::new(&mut mem, &mut || frames.alloc()).unwrap();
        let cell = start_at(
            9,
            &layout,
            elf::Machine::X86_64,
            pt,
            Some(&kernel),
            &mut mem,
            &mut frames,
            &[b"prog"],
            &[],
            [0; 16],
        )
        .unwrap();
        assert_eq!(
            (cell.regs().pc, cell.brk, cell.mmap_next),
            (MMAP_BASE, PIE_BASE + 0x6000, MMAP_BASE + 0x4000)
        );
        let mut aux = [0u8; 160]; // argc, argv, envp and the auxiliary vector
        let pt = PageTable::<X86_64>::from_root(cell.root);
        memory::copy_from(&pt, &mem, cell.regs().sp, &mut aux).unwrap();
        let words: Vec<u64> = aux
            .chunks(8)
            .map(|w| u64::from_le_bytes(w.try_into().unwrap()))
            .collect();
        assert!(words.windows(2).any(|w| w == [7, MMAP_BASE]), "AT_BASE");
        assert!(
            words.windows(2).any(|w| w == [9, PIE_BASE + 0x1000]),
            "AT_ENTRY"
        );
        space::destroy(pt, &mut mem, &mut frames);
        let held = cache.borrow().len() + 1; // the cache's pages, the kernel's root
        assert_eq!(
            frames.outstanding(),
            held,
            "every frame of the cell returned"
        );

        // Out of memory part way: nothing is left behind but the cache.
        for limit in 0..40 {
            let before = frames.outstanding();
            frames.limit = before + limit;
            let r = map_program::<X86_64, _, _>(
                &main_image,
                Some(&ld_image),
                &mut mem,
                &mut tlb,
                &mut frames,
                &mut page,
                &mut |_| {},
            );
            frames.limit = usize::MAX;
            match r {
                Ok((pt, _)) => space::destroy(pt, &mut mem, &mut frames),
                Err(e) => assert_eq!(e, elf::ElfError::OutOfMemory),
            }
            assert_eq!(frames.outstanding(), before, "limit {limit}");
        }
    }

    #[test]
    fn cell_state_copies_bit_for_bit() {
        let mut cell = Cell::new(7, 0x8000, 0x40_1000, STACK_TOP - 64, 0x60_0000);
        cell.regs_mut().gpr[3] = 0xdead;
        let regs = *cell.regs();
        assert_eq!(regs, *cell.regs());
        assert_eq!(regs.pc, 0x40_1000);
        let copy = cell.clone();
        assert_eq!(copy, cell);
        assert_eq!(copy.brk, copy.brk_start);
    }
}
