//! Starting a program from the cell's files (spec `cells`, dynamically
//! linked executables): the executable, and the interpreter it names, mapped
//! from their files' pages as Linux's `execve` maps them.

use crate::pages::FilePages;
use core::cell::RefCell;
use rhizo_cell::elf::{ElfError, Image, Machine};
use rhizo_cell::space::Frames;
use rhizo_cell::{Cell, map_program, start_at};
use rhizo_fs::Fs;
use rhizo_hal::{PhysicalMemory, Tlb};
use rhizo_mm::paging::{EntryFormat, PageTable};

/// Why a program cannot start from the cell's files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecError {
    /// The executable's path names no regular file.
    File(rhizo_fs::Error),
    /// The executable (or its interpreter: [`ElfError::BadInterpreter`])
    /// cannot run.
    Elf(ElfError),
}

impl From<ElfError> for ExecError {
    fn from(e: ElfError) -> Self {
        Self::Elf(e)
    }
}

/// Starts cell `id` running the executable at `path` in `fs` with `argv`
/// and `envp`: maps it and its interpreter (`PT_INTERP`, also from `fs`)
/// with their pages from `pages`, shares the kernel half of `kernel` when
/// given, and builds the stack and the first thread. On failure every frame
/// is returned.
#[allow(clippy::too_many_arguments)]
pub fn exec<F: EntryFormat, M: PhysicalMemory, A: Frames>(
    id: u32,
    machine: Machine,
    fs: &Fs,
    path: &[u8],
    kernel: Option<&PageTable<F>>,
    mem: &mut M,
    tlb: &mut impl Tlb,
    frames: &mut A,
    pages: &mut dyn FilePages<M, A>,
    argv: &[&[u8]],
    envp: &[&[u8]],
    random: [u8; 16],
) -> Result<Cell, ExecError> {
    let (ino, size) = fs.file_at(path).map_err(ExecError::File)?;
    let head = fs.page(ino, 0).map_err(ExecError::File)?;
    let main = Image::parse_file(head.bytes(), size, machine)?;
    let mut interp_ino = ino;
    let interp_head;
    let interp = match main.interpreter() {
        None => None,
        Some(path) => {
            let (ino, size) = fs.file_at(path).map_err(|_| ElfError::BadInterpreter)?;
            interp_ino = ino;
            interp_head = fs.page(ino, 0).map_err(|_| ElfError::BadInterpreter)?;
            let image = Image::parse_file(interp_head.bytes(), size, machine);
            Some(image.map_err(|_| ElfError::BadInterpreter)?)
        }
    };
    // Both callbacks reach the page source.
    let pages = RefCell::new(pages);
    let (pt, layout) = map_program::<F, M, A>(
        &main,
        interp.as_ref(),
        mem,
        tlb,
        frames,
        &mut |interp, i, mem, frames| {
            let page = fs.page(if interp { interp_ino } else { ino }, i).ok()?;
            pages.borrow_mut().frame(&page, mem, frames)
        },
        &mut |phys| pages.borrow_mut().code(phys),
    )?;
    Ok(start_at(
        id, &layout, machine, pt, kernel, mem, frames, argv, envp, random,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PageCache;
    use rhizo_cell::testing::{CountingFrames, elf_with};
    use rhizo_cell::{MMAP_BASE, PIE_BASE, memory, space};
    use rhizo_hal::testing::{FakePhysicalMemory, RecordingTlb};
    use rhizo_mm::paging::Aarch64;

    struct Machine {
        fs: Fs,
        mem: FakePhysicalMemory,
        frames: CountingFrames,
        cache: PageCache,
    }

    impl Machine {
        fn exec(&mut self, path: &[u8]) -> Result<Cell, ExecError> {
            exec::<Aarch64, _, _>(
                7,
                rhizo_cell::elf::Machine::Aarch64,
                &self.fs,
                path,
                None,
                &mut self.mem,
                &mut RecordingTlb::default(),
                &mut self.frames,
                &mut self.cache,
                &[path],
                &[b"LD_LIBRARY_PATH=/opt"],
                [1; 16],
            )
        }

        fn read(&self, cell: &Cell, at: u64) -> [u8; 4] {
            let mut out = [0; 4];
            let pt = PageTable::<Aarch64>::from_root(cell.root);
            memory::copy_from(&pt, &self.mem, at, &mut out).unwrap();
            out
        }
    }

    /// A program and its interpreter start from the cell's files; two
    /// cells of it share its pages; what cannot start says why and leaves
    /// nothing behind.
    #[test]
    fn programs_start_from_the_cells_files() {
        let ld_path = b"/lib/ld-linux-aarch64.so.1";
        let prog = elf_with(
            183,
            3,
            &[(0x1000, 5, b"MAIN", 4), (0x2000, 6, b"D", 0x2000)],
            Some(ld_path),
        );
        let ld = elf_with(
            183,
            3,
            &[(0, 5, b"LDSO", 4), (0x1000, 6, b"L", 0x1000)],
            None,
        );
        let mut fs = Fs::new();
        fs.put_file(b"/usr/bin/prog", &prog).unwrap();
        fs.put_file(ld_path, &ld).unwrap();
        let fixed = elf_with(183, 2, &[(0x40_0000, 5, b"STAT", 4)], None);
        fs.put_file(b"/usr/bin/static", &fixed).unwrap();
        let orphan = elf_with(183, 3, &[(0x1000, 5, b"O", 1)], Some(b"/lib/missing.so"));
        fs.put_file(b"/usr/bin/orphan", &orphan).unwrap();
        fs.put_file(b"/etc/passwd", b"root:x:0:0::/:/bin/sh")
            .unwrap();
        let mut m = Machine {
            fs,
            mem: FakePhysicalMemory::new(),
            frames: CountingFrames::new(1024),
            cache: PageCache::new(64),
        };

        let a = m.exec(b"/usr/bin/prog").unwrap();
        let b = m.exec(b"/usr/bin/prog").unwrap();
        assert_eq!(a.regs().pc, MMAP_BASE, "the interpreter runs first");
        assert_eq!(
            (a.brk, a.mmap_next),
            (PIE_BASE + 0x4000, MMAP_BASE + 0x2000)
        );
        assert_eq!(&m.read(&a, PIE_BASE + 0x1000), b"MAIN");
        assert_eq!(&m.read(&a, MMAP_BASE), b"LDSO");
        let code = |c: &Cell| {
            PageTable::<Aarch64>::from_root(c.root)
                .translate(&m.mem, MMAP_BASE)
                .unwrap()
                .phys
        };
        assert_eq!(code(&a), code(&b), "one frame for both cells");
        let s = m.exec(b"/usr/bin/static").unwrap();
        assert_eq!((s.regs().pc, s.mmap_next), (0x40_0000, MMAP_BASE));
        assert_eq!(&m.read(&s, 0x40_0000), b"STAT");

        let held = m.frames.outstanding();
        let refused = [
            (
                &b"/usr/bin/orphan"[..],
                ExecError::Elf(ElfError::BadInterpreter),
            ),
            (b"/usr/bin/none", ExecError::File(rhizo_fs::Error::NotFound)),
            (b"/usr/bin", ExecError::File(rhizo_fs::Error::IsDir)),
            (b"/etc/passwd", ExecError::Elf(ElfError::NotElf)),
        ];
        for (path, error) in refused {
            assert_eq!(m.exec(path).unwrap_err(), error);
        }
        assert_eq!(m.frames.outstanding(), held, "nothing left behind");

        for cell in [a, b, s] {
            space::destroy(
                PageTable::<Aarch64>::from_root(cell.root),
                &mut m.mem,
                &mut m.frames,
            );
        }
        m.cache.clear(&mut m.frames);
        assert_eq!(m.frames.outstanding(), 0);
    }

    /// Finding S18: an entry point past the cell's addresses, the
    /// executable's or its interpreter's, is refused (entering there would
    /// fault in the kernel on x86_64) and leaves nothing behind.
    #[test]
    fn entry_points_outside_the_cell_are_refused() {
        let ld_path = b"/lib/ld.so";
        let bad = 0x40_0000_0000_0000u64;
        let mut ld = elf_with(183, 3, &[(0, 5, b"LDSO", 4)], None);
        ld[24..32].copy_from_slice(&bad.to_le_bytes());
        let prog = elf_with(183, 3, &[(0x1000, 5, b"MAIN", 4)], Some(ld_path));
        let mut fixed = elf_with(183, 2, &[(0x40_0000, 5, b"STAT", 4)], None);
        fixed[24..32].copy_from_slice(&0x8000_0000_0000u64.to_le_bytes());
        let mut fs = Fs::new();
        fs.put_file(b"/prog", &prog).unwrap();
        fs.put_file(ld_path, &ld).unwrap();
        fs.put_file(b"/static", &fixed).unwrap();
        let mut m = Machine {
            fs,
            mem: FakePhysicalMemory::new(),
            frames: CountingFrames::new(1024),
            cache: PageCache::new(64),
        };
        assert_eq!(
            m.exec(b"/prog").unwrap_err(),
            ExecError::Elf(ElfError::BadEntry(bad.wrapping_add(MMAP_BASE)))
        );
        assert_eq!(
            m.exec(b"/static").unwrap_err(),
            ExecError::Elf(ElfError::BadEntry(0x8000_0000_0000))
        );
        m.cache.clear(&mut m.frames);
        assert_eq!(m.frames.outstanding(), 0, "nothing left behind");
    }

    proptest::proptest! {
        /// A program and an interpreter with any bytes changed (the cell
        /// may have written both): `exec` refuses them leaving nothing
        /// behind, or starts a cell whose pages are its own, in the lower
        /// half, never writable and executable at once, and all given back
        /// when it ends.
        #[test]
        fn tampered_programs_start_safely_or_not_at_all(
            prog_edits in proptest::collection::vec((0usize..0x900, proptest::prelude::any::<u8>()), 0..4),
            ld_edits in proptest::collection::vec((0usize..0x900, proptest::prelude::any::<u8>()), 0..4),
        ) {
            let ld_path = b"/lib/ld.so";
            let mut prog = elf_with(
                183,
                3,
                &[(0x1000, 5, b"MAIN", 4), (0x2000, 6, b"D", 0x2000)],
                Some(ld_path),
            );
            let mut ld = elf_with(183, 3, &[(0, 5, b"LDSO", 4), (0x1000, 6, b"L", 0x1000)], None);
            for (bytes, edits) in [(&mut prog, &prog_edits), (&mut ld, &ld_edits)] {
                for &(at, b) in edits {
                    bytes[at] = b;
                }
            }
            let mut fs = Fs::new();
            fs.put_file(b"/prog", &prog).unwrap();
            fs.put_file(ld_path, &ld).unwrap();
            let mut m = Machine {
                fs,
                mem: FakePhysicalMemory::new(),
                frames: CountingFrames::new(1 << 16),
                cache: PageCache::new(64),
            };
            m.frames.limit = 256;
            if let Ok(cell) = m.exec(b"/prog") {
                let pt = PageTable::<Aarch64>::from_root(cell.root);
                pt.for_each_lower_leaf(&m.mem, &mut |virt, _, size, flags| {
                    assert!(virt + size.bytes() <= rhizo_cell::USER_END, "{virt:#x}");
                    assert!(!(flags.is_writable() && flags.is_executable()), "W^X at {virt:#x}");
                });
                proptest::prop_assert!(cell.regs().pc < rhizo_cell::USER_END);
                space::destroy(pt, &mut m.mem, &mut m.frames);
            }
            m.cache.clear(&mut m.frames);
            proptest::prop_assert_eq!(m.frames.outstanding(), 0, "every frame back");
        }
    }
}
