//! Agent cells: the cell model, its address space and loading of static
//! Linux executables. Everything works through `rhizo-hal` interfaces and
//! `rhizo-mm` page tables, so it is tested on the host.

#![cfg_attr(not(any(test, feature = "testing")), no_std)]

pub mod elf;
pub mod memory;
pub mod space;

use rhizo_hal::PhysicalMemory;
use rhizo_mm::paging::{EntryFormat, PageTable};
use space::Frames;

/// Top of a cell's stack (exclusive); the stack grows down from here.
pub const STACK_TOP: u64 = 0x0000_7fff_ffff_f000;
/// Stack pages mapped when a cell starts.
pub const STACK_PAGES: u64 = 16;
/// First address used for anonymous mappings (`mmap`).
pub const MMAP_BASE: u64 = 0x0000_4000_0000_0000;

/// Everything needed to resume a cell's execution, as plain data on both
/// architectures (design decision D2). The AArch64 entry code saves and
/// restores it in place, so its layout is fixed: `pc` at 0, `sp` at 8, `gpr`
/// at 16, `tls` at 264, `flags` at 272.
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
}

/// A cell: plain data, no pointers into kernel memory (spec `cells`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    /// Identifier, unique while the cell exists.
    pub id: u32,
    /// Physical address of the cell's root page table.
    pub root: u64,
    /// Saved user registers.
    pub regs: Registers,
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
    /// A new cell rooted at `root`, about to start at `entry` with stack `sp`.
    pub fn new(id: u32, root: u64, entry: u64, sp: u64, brk_start: u64) -> Self {
        Self {
            id,
            root,
            regs: Registers {
                pc: entry,
                sp,
                ..Registers::default()
            },
            brk_start,
            brk: brk_start,
            mmap_next: MMAP_BASE,
            exit_status: None,
        }
    }
}

/// Creates cell `id` from the static executable `data`: a new hierarchy
/// (sharing the kernel half of `kernel` when given, as x86_64 needs), the
/// loaded image and the initial stack. On failure every frame is returned.
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
    let pt =
        PageTable::<F>::new(mem, &mut || frames.alloc()).map_err(|_| elf::ElfError::OutOfMemory)?;
    if let Some(kernel) = kernel {
        pt.share_upper_half(mem, kernel);
    }
    let built = image.load(&pt, mem, &mut || frames.alloc()).and_then(|()| {
        elf::build_stack(&pt, mem, &mut || frames.alloc(), &image, argv, envp, random)
    });
    match built {
        Ok(sp) => Ok(Cell::new(id, pt.root(), image.entry, sp, image.end())),
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
            self.live
                .get_mut(&phys)
                .unwrap_or_else(|| panic!("share of free {phys:#x}"))
                .1 += 1;
        }

        fn is_shared(&self, phys: u64) -> bool {
            self.live.get(&phys).is_some_and(|e| e.1 > 1)
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
        assert_eq!(core::mem::align_of::<super::Registers>(), 16);
    }

    use super::*;

    use crate::testing::CountingFrames;
    use rhizo_hal::testing::FakePhysicalMemory;
    use rhizo_mm::paging::X86_64;

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
        assert_eq!((cell.id, cell.regs.pc, cell.brk), (3, 0x40_0000, 0x40_3000));
        assert!(cell.regs.sp < STACK_TOP && cell.regs.sp.is_multiple_of(16));
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

    #[test]
    fn cell_state_copies_bit_for_bit() {
        let mut cell = Cell::new(7, 0x8000, 0x40_1000, STACK_TOP - 64, 0x60_0000);
        cell.regs.gpr[3] = 0xdead;
        let copy = cell;
        assert_eq!(copy, cell);
        assert_eq!(copy.regs.pc, 0x40_1000);
        assert_eq!(copy.brk, copy.brk_start);
    }
}
