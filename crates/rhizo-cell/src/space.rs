//! A cell's address space: anonymous mappings (with 2 MiB pages when the
//! region allows it), unmapping, and teardown that returns every frame.

use crate::USER_END;
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
    /// The range reaches past the cell's addresses ([`USER_END`]).
    NotUser,
}

/// Whether `addr..addr + len` lies within the cell's addresses.
fn is_user(addr: u64, len: u64) -> bool {
    addr.checked_add(len).is_some_and(|end| end <= USER_END)
}

/// Maps `len` bytes at `addr` (both 4 KiB aligned) to fresh zeroed memory with
/// `flags`. 2 MiB-aligned 2 MiB pieces use 2 MiB pages when a 2 MiB frame is
/// available. On failure nothing stays mapped and every frame is returned.
/// A range past [`USER_END`] is refused (`NonCanonical`): it would reach the
/// kernel half.
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
    if !is_user(addr, len) {
        return Err(MapError::NonCanonical(addr));
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

/// Unmaps every page in `addr..addr + len` (holes are skipped, a whole empty
/// table entry at a time) and returns its frames. Fails, changing nothing,
/// if the range would split a 2 MiB page or reaches past [`USER_END`].
pub fn unmap<F: EntryFormat>(
    pt: &PageTable<F>,
    mem: &mut impl PhysicalMemory,
    tlb: &mut impl Tlb,
    frames: &mut impl Frames,
    addr: u64,
    len: u64,
) -> Result<(), UnmapError> {
    if !is_user(addr, len) {
        return Err(UnmapError::NotUser);
    }
    let end = addr + len;
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
            None => at = pt.hole_end(mem, at).unwrap_or(at + FRAME_SIZE),
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
            Err(_) => at = pt.hole_end(mem, at).unwrap_or(at + FRAME_SIZE),
        }
    }
    Ok(())
}

/// Changes the permissions of every page in `addr..addr + len` (spec
/// `linux-abi`, `mprotect`). Fails, changing nothing, if a page in the range
/// is not mapped (anything past [`USER_END`] counts as not mapped) or the
/// range splits a 2 MiB page.
pub fn protect<F: EntryFormat>(
    pt: &PageTable<F>,
    mem: &mut impl PhysicalMemory,
    tlb: &mut impl Tlb,
    frames: &mut impl Frames,
    addr: u64,
    len: u64,
    flags: PageFlags,
) -> Result<(), ProtectError> {
    if !is_user(addr, len) {
        return Err(ProtectError::NotMapped(addr.max(USER_END)));
    }
    let end = addr + len;
    let mut at = addr;
    while at < end {
        let t = pt.translate(mem, at).ok_or(ProtectError::NotMapped(at))?;
        let base = at & !(t.size.bytes() - 1);
        if base < addr || base + t.size.bytes() > end {
            return Err(ProtectError::PartialLargePage(base));
        }
        at = base + t.size.bytes();
    }
    let mut at = addr;
    while at < end {
        pt.unshare_path(mem, frames, at)
            .map_err(|_| ProtectError::OutOfMemory)?;
        // A frame other cells still use (a read-only page after a fork)
        // becomes writable only through a private copy (spec `isolation`).
        let shared = pt
            .translate(mem, at)
            .is_some_and(|t| frames.is_shared(t.phys & !(t.size.bytes() - 1)));
        let flags = if shared && flags.is_writable() {
            flags | PageFlags::COPY_ON_WRITE
        } else {
            flags
        };
        let size = pt
            .protect(mem, tlb, at, flags)
            .map_err(|_| ProtectError::NotMapped(at))?;
        at = (at & !(size.bytes() - 1)) + size.bytes();
    }
    Ok(())
}

/// Why permissions cannot be changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectError {
    /// A page in the range is not mapped.
    NotMapped(u64),
    /// The range covers only part of a 2 MiB page.
    PartialLargePage(u64),
    /// No memory to make a shared page table private.
    OutOfMemory,
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

    /// A cell sharing a kernel half (as on x86_64): nothing here reaches it,
    /// and a huge empty range costs one step per empty table entry.
    fn the_kernel_half_is_out_of_reach<F: EntryFormat>() {
        let (pt, mut mem, mut tlb, mut frames) = setup::<F>();
        let kernel = PageTable::<F>::new(&mut mem, &mut || frames.alloc()).unwrap();
        let k = frames.alloc().unwrap();
        kernel
            .map(
                &mut mem,
                &mut || frames.alloc(),
                0xffff_8000_0000_0000,
                k,
                PageSize::Small,
                PageFlags::WRITABLE,
            )
            .unwrap();
        pt.share_upper_half(&mut mem, &kernel);
        let before = mem.clone();
        let kaddr = 0xffff_8000_0000_0000;
        assert_eq!(
            unmap(&pt, &mut mem, &mut tlb, &mut frames, kaddr, FRAME_SIZE),
            Err(UnmapError::NotUser)
        );
        assert_eq!(
            protect(&pt, &mut mem, &mut tlb, &mut frames, kaddr, FRAME_SIZE, RW),
            Err(ProtectError::NotMapped(kaddr))
        );
        assert_eq!(
            map_anonymous(&pt, &mut mem, &mut tlb, &mut frames, kaddr, FRAME_SIZE, RW),
            Err(MapError::NonCanonical(kaddr))
        );
        assert_eq!(
            map_anonymous(
                &pt,
                &mut mem,
                &mut tlb,
                &mut frames,
                USER_END,
                FRAME_SIZE,
                RW
            ),
            Err(MapError::NonCanonical(USER_END))
        );
        assert!(mem == before, "nothing changed");
        // The whole user half, almost all of it empty: done at once.
        map_anonymous(
            &pt,
            &mut mem,
            &mut tlb,
            &mut frames,
            0x7000_0000,
            0x3000,
            RW,
        )
        .unwrap();
        unmap(&pt, &mut mem, &mut tlb, &mut frames, 0, USER_END).unwrap();
        assert!(pt.translate(&mem, 0x7000_1000).is_none());
    }

    /// A read-only page shared with a fork (code, constants) made writable by
    /// one cell stays shared until written: the write lands in a private
    /// copy, never in the page the other cells still use (spec `isolation`).
    fn protect_never_hands_out_a_shared_page<F: EntryFormat>() {
        let (pt, mut mem, mut tlb, mut frames) = setup::<F>();
        let ro = PageFlags::USER;
        map_anonymous(&pt, &mut mem, &mut tlb, &mut frames, 0x40_0000, 0x1000, ro).unwrap();
        let page = pt.translate(&mem, 0x40_0000).unwrap().phys;
        mem.write_u64(page, 0xc0de);
        let child = pt.fork_lower_half(&mut mem, &mut frames).unwrap();
        protect(
            &child,
            &mut mem,
            &mut tlb,
            &mut frames,
            0x40_0000,
            0x1000,
            RW,
        )
        .unwrap();
        // A write as the kernel makes it: resolve the fault if refused.
        let t = child.translate(&mem, 0x40_0000).unwrap();
        if !t.flags.is_writable() {
            assert_eq!(
                child.resolve_write(&mut mem, &mut frames, &mut tlb, 0x40_0000),
                Ok(true)
            );
        }
        let t = child.translate(&mem, 0x40_0000).unwrap();
        assert!(t.flags.is_writable());
        mem.write_u64(t.phys, 0xbad);
        assert_eq!(mem.read_u64(page), 0xc0de, "the other cell's page");
        assert_eq!(pt.translate(&mem, 0x40_0000).unwrap().phys, page);
        destroy(child, &mut mem, &mut frames);
        destroy(pt, &mut mem, &mut frames);
        assert_eq!(frames.outstanding(), 0);
    }

    macro_rules! both_formats {
        ($($name:ident),*) => {
            mod aarch64 { $(#[test] fn $name() { super::$name::<super::Aarch64>(); })* }
            mod x86_64 { $(#[test] fn $name() { super::$name::<super::X86_64>(); })* }
        };
    }
    fn protect_needs_every_page_mapped<F: EntryFormat>() {
        let (pt, mut mem, mut tlb, mut frames) = setup::<F>();
        map_anonymous(&pt, &mut mem, &mut tlb, &mut frames, 0x1000, 0x3000, RW).unwrap();
        let ro = PageFlags::USER | PageFlags::NO_EXECUTE;
        assert_eq!(
            protect(&pt, &mut mem, &mut tlb, &mut frames, 0x1000, 0x4000, ro),
            Err(ProtectError::NotMapped(0x4000))
        );
        assert!(
            pt.translate(&mem, 0x1000).unwrap().flags.is_writable(),
            "nothing changed"
        );
        protect(&pt, &mut mem, &mut tlb, &mut frames, 0x2000, 0x2000, ro).unwrap();
        assert!(pt.translate(&mem, 0x1000).unwrap().flags.is_writable());
        assert!(!pt.translate(&mem, 0x3fff).unwrap().flags.is_writable());
    }

    both_formats!(
        the_kernel_half_is_out_of_reach,
        protect_never_hands_out_a_shared_page,
        protect_needs_every_page_mapped,
        large_regions_use_large_pages,
        teardown_returns_every_frame,
        memory_starts_zeroed_and_unmap_refuses_to_split,
        out_of_memory_rolls_back
    );
}
