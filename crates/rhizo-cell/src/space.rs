//! A cell's address space: anonymous mappings (with 2 MiB pages when the
//! region allows it), unmapping, and teardown that returns every frame.

use crate::{STACK_LIMIT, STACK_TOP, USER_END};
use alloc::boxed::Box;
use alloc::vec::Vec;
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

/// Maps `len` bytes at `addr` (both 4 KiB aligned) with `flags`, page `i` to
/// the frame `frame(i)` gives with a reference the mapping takes over
/// (`None`: no memory). A frame another mapping still uses is mapped
/// copy-on-write when `flags` is writable (spec `isolation`). On failure
/// nothing stays mapped and every reference is returned.
#[allow(clippy::too_many_arguments)]
pub fn map_frames<F: EntryFormat, M: PhysicalMemory, A: Frames>(
    pt: &PageTable<F>,
    mem: &mut M,
    tlb: &mut impl Tlb,
    frames: &mut A,
    addr: u64,
    len: u64,
    flags: PageFlags,
    frame: &mut impl FnMut(u64, &mut M, &mut A) -> Option<u64>,
) -> Result<(), MapError> {
    if !addr.is_multiple_of(FRAME_SIZE) || !len.is_multiple_of(FRAME_SIZE) {
        return Err(MapError::Misaligned);
    }
    if !is_user(addr, len) {
        return Err(MapError::NonCanonical(addr));
    }
    let read_only = [PageFlags::USER, PageFlags::NO_EXECUTE]
        .into_iter()
        .filter(|&f| flags.contains(f))
        .fold(PageFlags::empty(), PageFlags::union);
    let mut done = 0;
    let result = loop {
        if done == len {
            break Ok(());
        }
        let at = addr + done;
        if let Err(e) = pt.unshare_path(mem, frames, at) {
            break Err(e);
        }
        let Some(phys) = frame(done / FRAME_SIZE, mem, frames) else {
            break Err(MapError::OutOfFrames);
        };
        let shared = frames.is_shared(phys);
        let first = if shared { read_only } else { flags };
        if let Err(e) = pt.map(
            mem,
            &mut || frames.alloc(),
            at,
            phys,
            PageSize::Small,
            first,
        ) {
            frames.free(phys, PageSize::Small);
            break Err(e);
        }
        if shared && flags.is_writable() {
            let _ = pt.protect(mem, tlb, at, flags | PageFlags::COPY_ON_WRITE);
        }
        done += FRAME_SIZE;
    };
    if result.is_err() {
        let _ = unmap(pt, mem, tlb, frames, addr, done);
    }
    result
}

/// Grows the stack as Linux does on a fault: maps a zeroed, writable page at
/// `addr` when it lies within [`STACK_LIMIT`] below [`STACK_TOP`] and is not
/// mapped yet. Returns whether it did (`false` also when no frame is left).
pub fn grow_stack<F: EntryFormat>(
    pt: &PageTable<F>,
    mem: &mut impl PhysicalMemory,
    tlb: &mut impl Tlb,
    frames: &mut impl Frames,
    addr: u64,
) -> bool {
    let rw = PageFlags::USER | PageFlags::WRITABLE | PageFlags::NO_EXECUTE;
    (STACK_TOP - STACK_LIMIT..STACK_TOP).contains(&addr)
        && pt.translate(mem, addr).is_none()
        && map_anonymous(
            pt,
            mem,
            tlb,
            frames,
            addr & !(FRAME_SIZE - 1),
            FRAME_SIZE,
            rw,
        )
        .is_ok()
}

/// The permissions of a write-or-execute page as it starts: writing
/// (change `node-jit`).
pub const WX_WRITING: PageFlags = PageFlags::USER
    .union(PageFlags::WRITABLE)
    .union(PageFlags::NO_EXECUTE)
    .union(PageFlags::WRITE_OR_EXEC);

/// Most write-or-execute ranges a cell may have (spec `isolation`).
pub const MAX_WX_RANGES: usize = 64;

/// A cell's write-or-execute ranges (design D5 of `node-jit`): addresses it
/// asked writable and executable with no memory behind them yet, whose
/// pages come zeroed, writing, the first time they are touched. Sorted,
/// page aligned, never empty, never touching, at most [`MAX_WX_RANGES`].
/// One word while there are none, as for nearly every cell: making,
/// forking and dropping a cell costs nothing more.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[allow(
    clippy::box_collection,
    reason = "one word in every live cell, not three: the extra allocation is only for cells with ranges"
)]
pub struct WxRanges(Option<Box<Vec<(u64, u64)>>>);

/// More write-or-execute ranges than [`MAX_WX_RANGES`] would be needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooManyRanges;

impl WxRanges {
    /// The ranges `ranges` (`(start, end)` pairs), if they keep the rules
    /// above and lie within the cell's addresses.
    pub fn from_ranges(ranges: Vec<(u64, u64)>) -> Option<Self> {
        let aligned = |a: u64| a.is_multiple_of(FRAME_SIZE);
        let valid = ranges.len() <= MAX_WX_RANGES
            && ranges
                .iter()
                .all(|&(s, e)| aligned(s) && aligned(e) && s < e && e <= USER_END)
            && ranges.windows(2).all(|w| w[0].1 < w[1].0);
        valid.then(|| Self::of(ranges))
    }

    fn of(ranges: Vec<(u64, u64)>) -> Self {
        Self((!ranges.is_empty()).then(|| Box::new(ranges)))
    }

    /// The ranges, in address order.
    pub fn ranges(&self) -> &[(u64, u64)] {
        self.0.as_deref().map_or(&[], Vec::as_slice)
    }

    /// Whether `addr` lies in a range.
    pub fn contains(&self, addr: u64) -> bool {
        self.ranges().iter().any(|&(s, e)| (s..e).contains(&addr))
    }

    /// Adds `start..end` (page aligned), merged with the ranges it overlaps
    /// or touches. Changes nothing if that would make too many.
    pub fn add(&mut self, start: u64, end: u64) -> Result<(), TooManyRanges> {
        if start >= end {
            return Ok(());
        }
        let list = self.ranges();
        let first = list.partition_point(|&(_, e)| e < start);
        let last = list.partition_point(|&(s, _)| s <= end);
        if first == last && list.len() == MAX_WX_RANGES {
            return Err(TooManyRanges);
        }
        let merged = list[first..last]
            .iter()
            .fold((start, end), |(s, e), &(rs, re)| (s.min(rs), e.max(re)));
        self.0.get_or_insert_default().splice(first..last, [merged]);
        Ok(())
    }

    /// Removes `start..end` from the ranges. Changes nothing if splitting a
    /// range in two would make too many.
    pub fn remove(&mut self, start: u64, end: u64) -> Result<(), TooManyRanges> {
        let list = self.ranges();
        let first = list.partition_point(|&(_, e)| e <= start);
        let last = list.partition_point(|&(s, _)| s < end);
        if first == last {
            return Ok(());
        }
        let left = (list[first].0 < start).then(|| (list[first].0, start));
        let right = (list[last - 1].1 > end).then(|| (end, list[last - 1].1));
        let kept = left.iter().chain(right.iter()).count();
        if list.len() - (last - first) + kept > MAX_WX_RANGES {
            return Err(TooManyRanges);
        }
        let mut list = core::mem::take(self).0.map_or_else(Vec::new, |b| *b);
        list.splice(first..last, left.into_iter().chain(right));
        *self = Self::of(list);
        Ok(())
    }
}

/// Backs the page containing `addr` with a zeroed write-or-execute page,
/// writing, if it lies in one of `wx` and is not mapped (design D5 of
/// `node-jit`). Returns whether it did (`false` also when no frame is left).
pub fn back_wx<F: EntryFormat>(
    pt: &PageTable<F>,
    mem: &mut impl PhysicalMemory,
    tlb: &mut impl Tlb,
    frames: &mut impl Frames,
    wx: &WxRanges,
    addr: u64,
) -> bool {
    wx.contains(addr)
        && pt.translate(mem, addr).is_none()
        && map_anonymous(
            pt,
            mem,
            tlb,
            frames,
            addr & !(FRAME_SIZE - 1),
            FRAME_SIZE,
            WX_WRITING,
        )
        .is_ok()
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

    #[test]
    fn stack_grows_on_touch_within_its_limit() {
        let (pt, mut mem, mut tlb, mut frames) = setup::<X86_64>();
        let deep = STACK_TOP - STACK_LIMIT + 12;
        assert!(grow_stack(&pt, &mut mem, &mut tlb, &mut frames, deep));
        let t = pt.translate(&mem, deep).expect("mapped");
        assert!(t.flags.contains(PageFlags::WRITABLE | PageFlags::USER));
        let mut word = [1u8; 8];
        mem.read_bytes(t.phys, &mut word);
        assert_eq!(word, [0; 8], "zeroed");
        assert!(
            !grow_stack(&pt, &mut mem, &mut tlb, &mut frames, deep + 8),
            "already mapped"
        );
        for outside in [
            STACK_TOP - STACK_LIMIT - 1,
            STACK_TOP,
            0x1000,
            crate::MMAP_BASE,
        ] {
            assert!(!grow_stack(&pt, &mut mem, &mut tlb, &mut frames, outside));
            assert!(pt.translate(&mem, outside).is_none());
        }
        frames.limit = 0; // out of frames
        assert!(!grow_stack(
            &pt,
            &mut mem,
            &mut tlb,
            &mut frames,
            STACK_TOP - 8
        ));
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

    /// A page of a write-or-execute range comes zeroed, writing, in 4 KiB,
    /// once; a page outside comes from nowhere (design D5 of `node-jit`).
    fn wx_pages_come_on_first_touch<F: EntryFormat>() {
        let (pt, mut mem, mut tlb, mut frames) = setup::<F>();
        let mut wx = WxRanges::default();
        wx.add(0x40_0000, 0x40_0000 + (4 << 20)).unwrap();
        let before = frames.outstanding();
        assert!(back_wx(
            &pt,
            &mut mem,
            &mut tlb,
            &mut frames,
            &wx,
            0x60_0123
        ));
        let t = pt.translate(&mem, 0x60_0123).unwrap();
        assert_eq!(t.size, PageSize::Small);
        assert_eq!(t.flags, WX_WRITING);
        assert_eq!(mem.read_u64(t.phys & !0xfff), 0);
        assert!(
            !back_wx(&pt, &mut mem, &mut tlb, &mut frames, &wx, 0x60_0000),
            "mapped already"
        );
        assert!(!back_wx(
            &pt,
            &mut mem,
            &mut tlb,
            &mut frames,
            &wx,
            0x80_0000
        ));
        assert!(pt.translate(&mem, 0x80_0000).is_none());
        frames.limit = frames.outstanding();
        assert!(!back_wx(
            &pt,
            &mut mem,
            &mut tlb,
            &mut frames,
            &wx,
            0x40_0000
        ));
        frames.limit = usize::MAX;
        assert!(frames.outstanding() > before);
        destroy(pt, &mut mem, &mut frames);
        assert_eq!(frames.outstanding(), 0);
    }

    both_formats!(
        the_kernel_half_is_out_of_reach,
        protect_never_hands_out_a_shared_page,
        protect_needs_every_page_mapped,
        large_regions_use_large_pages,
        teardown_returns_every_frame,
        memory_starts_zeroed_and_unmap_refuses_to_split,
        out_of_memory_rolls_back,
        wx_pages_come_on_first_touch
    );

    #[test]
    fn wx_ranges_merge_split_and_stay_bounded() {
        const P: u64 = FRAME_SIZE;
        let mut wx = WxRanges::default();
        wx.add(10 * P, 20 * P).unwrap();
        wx.add(30 * P, 40 * P).unwrap();
        wx.add(20 * P, 25 * P).unwrap(); // touching: merged
        assert_eq!(wx.ranges(), &[(10 * P, 25 * P), (30 * P, 40 * P)]);
        wx.add(24 * P, 31 * P).unwrap(); // overlapping both
        assert_eq!(wx.ranges(), &[(10 * P, 40 * P)]);
        wx.remove(15 * P, 16 * P).unwrap(); // a hole in the middle
        assert_eq!(wx.ranges(), &[(10 * P, 15 * P), (16 * P, 40 * P)]);
        assert!(wx.contains(15 * P - 1) && !wx.contains(15 * P) && wx.contains(16 * P));
        wx.remove(0, 12 * P).unwrap();
        wx.remove(39 * P, 50 * P).unwrap();
        assert_eq!(wx.ranges(), &[(12 * P, 15 * P), (16 * P, 39 * P)]);
        wx.remove(0, 100 * P).unwrap();
        assert_eq!(wx, WxRanges::default());
        // At the bound: one more range, or a split, is refused unchanged.
        for i in 0..MAX_WX_RANGES as u64 {
            wx.add(4 * i * P, (4 * i + 2) * P).unwrap();
        }
        let full = wx.clone();
        assert_eq!(wx.add(1000 * P, 1001 * P), Err(TooManyRanges));
        assert_eq!(wx, full, "unchanged");
        wx.add(4 * P, 6 * P).unwrap(); // already there: no new range
        wx.add(2 * P, 4 * P).unwrap(); // joins two: one fewer
        assert_eq!(wx.ranges().len(), MAX_WX_RANGES - 1);
        let mut trimmed = full;
        trimmed.remove(0, P).unwrap(); // trims a range: as many
        assert_eq!(trimmed.ranges().len(), MAX_WX_RANGES);
        // Splitting one range in two at the bound is refused unchanged.
        let mut wide = WxRanges::default();
        wide.add(0, 8 * P).unwrap();
        for i in 1..MAX_WX_RANGES as u64 {
            wide.add(100 * P + 4 * i * P, 100 * P + (4 * i + 2) * P)
                .unwrap();
        }
        let before = wide.clone();
        assert_eq!(wide.remove(2 * P, 3 * P), Err(TooManyRanges));
        assert_eq!(wide, before);
        assert!(wide.add(0, 0).is_ok() && wide == before, "empty");
    }

    #[test]
    fn wx_ranges_from_outside_are_checked() {
        const P: u64 = FRAME_SIZE;
        let ok = |r: &[(u64, u64)]| WxRanges::from_ranges(r.to_vec()).is_some();
        assert!(ok(&[]) && ok(&[(P, 2 * P), (3 * P, 4 * P)]));
        assert!(!ok(&[(P, P)]), "empty");
        assert!(!ok(&[(1, P)]), "unaligned");
        assert!(!ok(&[(P, 2 * P), (2 * P, 3 * P)]), "touching");
        assert!(!ok(&[(3 * P, 4 * P), (P, 2 * P)]), "out of order");
        assert!(!ok(&[(P, USER_END + P)]), "past the cell");
        let many: Vec<_> = (0..=MAX_WX_RANGES as u64)
            .map(|i| (2 * i * P, (2 * i + 1) * P))
            .collect();
        assert!(!ok(&many), "too many");
        assert!(ok(&many[..MAX_WX_RANGES]));
    }
}
