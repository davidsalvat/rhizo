//! A cell's address space: anonymous mappings (with 2 MiB pages when the
//! region allows it), unmapping, and teardown that returns every frame.

use rhizo_hal::{PhysicalMemory, Tlb};
use rhizo_mm::paging::{EntryFormat, MapError, PageFlags, PageSize, PageTable};
use rhizo_mm::{FRAME_SIZE, LARGE_PAGE_SIZE};

pub use rhizo_mm::cow::Frames;

/// Why a range cannot be unmapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnmapError {
    /// The range covers only part of a 2 MiB page (splitting is not supported).
    PartialLargePage(u64),
    /// No memory to make a shared page table private.
    OutOfMemory,
}

/// Maps `len` bytes at `addr` (both 4 KiB aligned) to fresh zeroed memory with
/// `flags`. 2 MiB-aligned 2 MiB pieces use 2 MiB pages when a 2 MiB frame is
/// available. On failure nothing stays mapped and every frame is returned.
pub fn map_anonymous<F: EntryFormat>(
    pt: &PageTable<F>,
    mem: &mut impl PhysicalMemory,
    tlb: &mut impl Tlb,
    frames: &mut impl Frames,
    addr: u64,
    len: u64,
    flags: PageFlags,
) -> Result<(), MapError> {
    if !addr.is_multiple_of(FRAME_SIZE) || !len.is_multiple_of(FRAME_SIZE) {
        return Err(MapError::Misaligned);
    }
    let mut done = 0;
    let result = loop {
        if done == len {
            break Ok(());
        }
        let at = addr + done;
        let large = if at.is_multiple_of(LARGE_PAGE_SIZE) && len - done >= LARGE_PAGE_SIZE {
            frames.alloc_large()
        } else {
            None
        };
        let (phys, size) = match large {
            Some(phys) => (phys, PageSize::Large),
            None => match frames.alloc() {
                Some(phys) => (phys, PageSize::Small),
                None => break Err(MapError::OutOfFrames),
            },
        };
        for frame in (phys..phys + size.bytes()).step_by(FRAME_SIZE as usize) {
            mem.zero_frame(frame);
        }
        // The tables on the way may be shared with a forked cell.
        if let Err(e) = pt.unshare_path(mem, frames, at) {
            frames.free(phys, size);
            break Err(e);
        }
        if let Err(e) = pt.map(mem, &mut || frames.alloc(), at, phys, size, flags) {
            frames.free(phys, size);
            break Err(e);
        }
        done += size.bytes();
    };
    if result.is_err() {
        // Undo the pieces mapped so far; they never span a partial large page.
        let _ = unmap(pt, mem, tlb, frames, addr, done);
    }
    result
}

/// Unmaps every page in `addr..addr + len` (holes are skipped) and returns its
/// frames. Fails, changing nothing, if the range would split a 2 MiB page.
pub fn unmap<F: EntryFormat>(
    pt: &PageTable<F>,
    mem: &mut impl PhysicalMemory,
    tlb: &mut impl Tlb,
    frames: &mut impl Frames,
    addr: u64,
    len: u64,
) -> Result<(), UnmapError> {
    let end = addr.saturating_add(len);
    // First pass: refuse ranges that would split a 2 MiB page.
    let mut at = addr;
    while at < end {
        match pt.translate(mem, at) {
            Some(t) => {
                let base = at & !(t.size.bytes() - 1);
                if base < addr || base + t.size.bytes() > end {
                    return Err(UnmapError::PartialLargePage(base));
                }
                at = base + t.size.bytes();
            }
            None => at = (at & !(FRAME_SIZE - 1)) + FRAME_SIZE,
        }
    }
    let mut at = addr;
    while at < end {
        // Unmapping changes a table, which must not be shared.
        if pt.unshare_path(mem, frames, at).is_err() {
            return Err(UnmapError::OutOfMemory);
        }
        match pt.unmap(mem, tlb, at) {
            Ok((phys, size)) => {
                frames.free(phys, size);
                at = (at & !(size.bytes() - 1)) + size.bytes();
            }
            Err(_) => at = (at & !(FRAME_SIZE - 1)) + FRAME_SIZE,
        }
    }
    Ok(())
}

/// Frees the whole user half of `pt`: drops its reference to every mapped
/// page and table (freeing what no other cell shares), and the root.
pub fn destroy<F: EntryFormat>(
    pt: PageTable<F>,
    mem: &mut impl PhysicalMemory,
    frames: &mut impl Frames,
) {
    pt.destroy_lower_half(mem, frames);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::CountingFrames;
    use rhizo_hal::testing::{FakePhysicalMemory, RecordingTlb};
    use rhizo_mm::paging::{Aarch64, X86_64};

    const RW: PageFlags = PageFlags::USER
        .union(PageFlags::WRITABLE)
        .union(PageFlags::NO_EXECUTE);

    fn setup<F: EntryFormat>() -> (
        PageTable<F>,
        FakePhysicalMemory,
        RecordingTlb,
        CountingFrames,
    ) {
        let mut mem = FakePhysicalMemory::new();
        let mut frames = CountingFrames::new(1024);
        let pt = PageTable::new(&mut mem, &mut || frames.alloc()).unwrap();
        (pt, mem, RecordingTlb::default(), frames)
    }

    fn large_regions_use_large_pages<F: EntryFormat>() {
        let (pt, mut mem, mut tlb, mut frames) = setup::<F>();
        let base = 0x4000_0000;
        map_anonymous(&pt, &mut mem, &mut tlb, &mut frames, base, 4 << 20, RW).unwrap();
        for at in [base, base + (2 << 20)] {
            assert_eq!(pt.translate(&mem, at).unwrap().size, PageSize::Large);
        }
        // An unaligned region falls back to small pages.
        map_anonymous(&pt, &mut mem, &mut tlb, &mut frames, 0x1000, 2 << 20, RW).unwrap();
        assert_eq!(pt.translate(&mem, 0x1000).unwrap().size, PageSize::Small);
    }

    fn teardown_returns_every_frame<F: EntryFormat>() {
        let (pt, mut mem, mut tlb, mut frames) = setup::<F>();
        map_anonymous(&pt, &mut mem, &mut tlb, &mut frames, 0x20_0000, 6 << 20, RW).unwrap();
        map_anonymous(
            &pt,
            &mut mem,
            &mut tlb,
            &mut frames,
            0x7fff_0000_0000,
            0x5000,
            RW,
        )
        .unwrap();
        unmap(
            &pt,
            &mut mem,
            &mut tlb,
            &mut frames,
            0x7fff_0000_1000,
            0x1000,
        )
        .unwrap();
        destroy(pt, &mut mem, &mut frames);
        assert_eq!(frames.outstanding(), 0);
    }

    fn memory_starts_zeroed_and_unmap_refuses_to_split<F: EntryFormat>() {
        let (pt, mut mem, mut tlb, mut frames) = setup::<F>();
        let phys = frames.alloc_large().unwrap();
        mem.write_u64(phys + 0x1000, 0xdead); // stale data in a recycled frame
        frames.free(phys, PageSize::Large);
        map_anonymous(&pt, &mut mem, &mut tlb, &mut frames, 0x20_0000, 2 << 20, RW).unwrap();
        let t = pt.translate(&mem, 0x20_1000).unwrap();
        assert_eq!(mem.read_u64(t.phys), 0);
        assert_eq!(
            unmap(&pt, &mut mem, &mut tlb, &mut frames, 0x20_1000, 0x1000),
            Err(UnmapError::PartialLargePage(0x20_0000))
        );
        assert!(pt.translate(&mem, 0x20_1000).is_some(), "nothing changed");
    }

    fn out_of_memory_rolls_back<F: EntryFormat>() {
        let (pt, mut mem, mut tlb, mut frames) = setup::<F>();
        let before = frames.outstanding();
        frames.limit = before + 8;
        assert_eq!(
            map_anonymous(
                &pt,
                &mut mem,
                &mut tlb,
                &mut frames,
                0x1000,
                64 * 0x1000,
                RW
            ),
            Err(MapError::OutOfFrames)
        );
        assert!(pt.translate(&mem, 0x1000).is_none());
        // Only intermediate tables may remain (they belong to the hierarchy).
        frames.limit = usize::MAX;
        destroy(pt, &mut mem, &mut frames);
        assert_eq!(frames.outstanding(), 0);
    }

    macro_rules! both_formats {
        ($($name:ident),*) => {
            mod aarch64 { $(#[test] fn $name() { super::$name::<super::Aarch64>(); })* }
            mod x86_64 { $(#[test] fn $name() { super::$name::<super::X86_64>(); })* }
        };
    }
    both_formats!(
        large_regions_use_large_pages,
        teardown_returns_every_frame,
        memory_starts_zeroed_and_unmap_refuses_to_split,
        out_of_memory_rolls_back
    );
}
