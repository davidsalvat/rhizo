//! Copy-on-write that shares page tables as well as pages (design D2 of
//! `add-fork`).
//!
//! Invariant, kept by every operation here: an entry that points at a frame
//! with more than one reference (a table, or a page) forbids writes: a table
//! entry through its table-level permission, a leaf with the copy-on-write
//! mark. Forking therefore only has to mark the parent's top-level entries,
//! and a write fault copies only what it must, on the path to the page.

use crate::FRAME_SIZE;
use crate::paging::{EntryFormat, MapError, PageFlags, PageSize, PageTable};
use rhizo_hal::{PhysicalMemory, Tlb};

/// Where a cell's frames come from and go back to, with reference counts so
/// a frame (a page or a page table) can belong to several cells.
pub trait Frames {
    /// One 4 KiB frame (contents undefined), with one reference.
    fn alloc(&mut self) -> Option<u64>;
    /// 512 contiguous frames aligned to 2 MiB (contents undefined), with one
    /// reference.
    fn alloc_large(&mut self) -> Option<u64>;
    /// Drops one reference to a page obtained from `alloc` (`Small`) or
    /// `alloc_large` (`Large`); the page is freed with the last one.
    fn free(&mut self, phys: u64, size: PageSize);
    /// Adds a reference to the page or table at `phys`.
    fn share(&mut self, phys: u64);
    /// Whether the page or table at `phys` has more than one reference.
    fn is_shared(&self, phys: u64) -> bool;
    /// Adds a reference to the page at `phys` if its count can take one
    /// more; returns whether it did. Counts are bounded: a caller that a
    /// cell can make share one page without end uses this, and gives the
    /// cell a copy when it fails (finding S14 of `docs/security.md`).
    fn try_share(&mut self, phys: u64) -> bool {
        self.share(phys);
        true
    }
}

const ENTRIES: u64 = 512;
/// First address past the lower (user) half.
const LOWER_HALF_END: u64 = 0x0000_8000_0000_0000;

fn index(virt: u64, level: u32) -> u64 {
    (virt >> (12 + 9 * level)) & (ENTRIES - 1)
}

impl<F: EntryFormat> PageTable<F> {
    /// Forks the user half: a new hierarchy whose root shares every lower
    /// table of this one (and copies the upper half's entries, as x86_64
    /// cells need). Allocates only the new root. Writes that this hierarchy
    /// could make before are now refused until resolved by
    /// [`resolve_write`](Self::resolve_write): the caller flushes its TLB.
    pub fn fork_lower_half(
        &self,
        mem: &mut impl PhysicalMemory,
        frames: &mut impl Frames,
    ) -> Result<Self, MapError> {
        let child = frames.alloc().ok_or(MapError::OutOfFrames)?;
        for i in 0..ENTRIES {
            let entry = mem.read_u64(self.root() + i * 8);
            let entry = if i < ENTRIES / 2 && F::is_valid(entry) {
                frames.share(F::address(entry));
                let shared = F::with_table_writable(entry, false);
                mem.write_u64(self.root() + i * 8, shared);
                shared
            } else {
                entry
            };
            mem.write_u64(child + i * 8, entry);
        }
        Ok(Self::from_root(child))
    }

    /// Copies the pages of the user half within `span` (it must hold every
    /// page there is) into a new hierarchy with tables of its own (design
    /// D2 of `add-image-cache`): every read-only page is shared (one more
    /// reference), every writable or copy-on-write page is copied into a
    /// private frame. Only the table entries that cover `span` are read.
    /// This hierarchy is only read, so it may be cloned while others clone
    /// it too; the copy changes its own tables without affecting anyone. On
    /// failure every frame taken is returned.
    pub fn clone_lower_half(
        &self,
        mem: &mut impl PhysicalMemory,
        frames: &mut impl Frames,
        span: core::ops::Range<u64>,
    ) -> Result<Self, MapError> {
        #[allow(clippy::too_many_arguments)]
        fn walk<F: EntryFormat>(
            mem: &mut impl PhysicalMemory,
            frames: &mut impl Frames,
            from: u64,
            to: u64,
            level: u32,
            base: u64,
            entries: u64,
            span: &core::ops::Range<u64>,
        ) -> Result<(), MapError> {
            let size = 1u64 << (12 + 9 * level);
            let first = span.start.saturating_sub(base) / size;
            let last = span.end.saturating_sub(base).div_ceil(size).min(entries);
            for i in first..last {
                let entry = mem.read_u64(from + i * 8);
                if !F::is_valid(entry) {
                    continue;
                }
                if !F::is_leaf(entry, level) {
                    let table = frames.alloc().ok_or(MapError::OutOfFrames)?;
                    mem.zero_frame(table);
                    // Linked before it is filled, so a failure below frees it.
                    mem.write_u64(
                        to + i * 8,
                        F::with_table_writable(F::with_address(entry, table), true),
                    );
                    let at = base + i * size;
                    walk::<F>(
                        mem,
                        frames,
                        F::address(entry),
                        table,
                        level - 1,
                        at,
                        ENTRIES,
                        span,
                    )?;
                    continue;
                }
                let bytes = leaf_bytes::<F>(entry, level);
                let page = F::address(entry) & !(bytes - 1);
                let flags = F::flags(entry);
                if flags.is_writable() || flags.contains(PageFlags::COPY_ON_WRITE) {
                    let copy = match bytes {
                        FRAME_SIZE => frames.alloc(),
                        crate::LARGE_PAGE_SIZE => frames.alloc_large(),
                        _ => None,
                    }
                    .ok_or(MapError::OutOfFrames)?;
                    mem.copy(page, copy, bytes);
                    let private = F::with_cow(F::with_leaf_writable(entry, true), false);
                    mem.write_u64(to + i * 8, F::with_address(private, copy));
                } else {
                    frames.share(page);
                    mem.write_u64(to + i * 8, entry);
                }
            }
            Ok(())
        }
        let root = frames.alloc().ok_or(MapError::OutOfFrames)?;
        mem.zero_frame(root);
        let copy = Self::from_root(root);
        if let Err(e) = walk::<F>(mem, frames, self.root(), root, 3, 0, ENTRIES / 2, &span) {
            copy.destroy_lower_half(mem, frames);
            return Err(e);
        }
        Ok(copy)
    }

    /// Makes the tables on the way to `virt` private to this hierarchy and
    /// writable, down to the level above the leaf (or above the first empty
    /// entry). Called before changing a mapping, so the change is not seen
    /// by a hierarchy sharing these tables.
    pub fn unshare_path(
        &self,
        mem: &mut impl PhysicalMemory,
        frames: &mut impl Frames,
        virt: u64,
    ) -> Result<(), MapError> {
        self.walk_private(mem, frames, virt).map(|_| ())
    }

    /// Makes every table of the user half private to this hierarchy and
    /// writable, as writes everywhere would, without copying a page (design
    /// D1 of `add-shared-snapshots`): afterwards a page another hierarchy
    /// can reach counts that reference itself, and is copy-on-write (or
    /// read-only) everywhere, so it can be shared once more safely.
    pub fn unshare_tables(
        &self,
        mem: &mut impl PhysicalMemory,
        frames: &mut impl Frames,
    ) -> Result<(), MapError> {
        fn walk<F: EntryFormat>(
            mem: &mut impl PhysicalMemory,
            frames: &mut impl Frames,
            table: u64,
            level: u32,
            entries: u64,
        ) -> Result<(), MapError> {
            for i in 0..entries {
                let slot = table + i * 8;
                let entry = mem.read_u64(slot);
                if !F::is_valid(entry) || F::is_leaf(entry, level) {
                    continue;
                }
                let next = private_table::<F>(mem, frames, slot, entry, level)?;
                // A table of the last level holds only pages.
                if level > 1 {
                    walk::<F>(mem, frames, next, level - 1, ENTRIES)?;
                }
            }
            Ok(())
        }
        walk::<F>(mem, frames, self.root(), 3, ENTRIES / 2)
    }

    /// Resolves a write to `virt` refused only because of sharing: copies the
    /// shared tables on the path and the page if it still has other
    /// references, or makes it writable again if not. Returns `false` if the
    /// write is refused for a real reason (no page, or a read-only one). The
    /// caller flushes the TLB entry for `virt`.
    ///
    /// A copied page replaces the old one break-before-make: the entry is
    /// cleared and flushed through `tlb` before it points at the copy, so
    /// no CPU running the same tables holds both (AArch64 requires it when
    /// the output address changes; design D7 of `add-parallel-threads`).
    pub fn resolve_write(
        &self,
        mem: &mut impl PhysicalMemory,
        frames: &mut impl Frames,
        tlb: &mut impl Tlb,
        virt: u64,
    ) -> Result<bool, MapError> {
        let Some((slot, level)) = self.walk_private(mem, frames, virt)? else {
            return Ok(false);
        };
        let entry = mem.read_u64(slot);
        let flags = F::flags(entry);
        if flags.is_writable() {
            return Ok(true); // already resolved (a stale TLB entry faulted)
        }
        if !flags.contains(crate::paging::PageFlags::COPY_ON_WRITE) {
            return Ok(false);
        }
        let size = match level {
            0 => PageSize::Small,
            1 => PageSize::Large,
            _ => PageSize::Huge,
        };
        let page = F::address(entry) & !(size.bytes() - 1);
        let writable = F::with_cow(F::with_leaf_writable(entry, true), false);
        if frames.is_shared(page) {
            let copy = match size {
                PageSize::Small => frames.alloc(),
                _ => frames.alloc_large(),
            };
            let new = match copy {
                Some(copy) => {
                    mem.copy(page, copy, size.bytes());
                    F::with_address(writable, copy)
                }
                None if size == PageSize::Large => {
                    F::table(copy_as_small_pages::<F>(mem, frames, page, writable)?)
                }
                None => return Err(MapError::OutOfFrames),
            };
            mem.write_u64(slot, 0);
            tlb.flush_page(virt & !(size.bytes() - 1));
            mem.write_u64(slot, new);
            frames.free(page, size);
        } else {
            mem.write_u64(slot, writable);
        }
        Ok(true)
    }

    /// Walks to the leaf for `virt`, making every table on the way private
    /// and writable. Returns the leaf's slot and level, or `None` if there is
    /// no leaf (the walk stops at the first empty entry). The upper half is
    /// the kernel's, shared by every cell on x86_64: never walked, so no cell
    /// write or mapping change reaches it (spec `isolation`).
    fn walk_private(
        &self,
        mem: &mut impl PhysicalMemory,
        frames: &mut impl Frames,
        virt: u64,
    ) -> Result<Option<(u64, u32)>, MapError> {
        if virt >= LOWER_HALF_END {
            return Ok(None);
        }
        let mut table = self.root();
        for level in (0..=3).rev() {
            let slot = table + index(virt, level) * 8;
            let entry = mem.read_u64(slot);
            if !F::is_valid(entry) {
                return Ok(None);
            }
            if F::is_leaf(entry, level) {
                return Ok(Some((slot, level)));
            }
            table = private_table::<F>(mem, frames, slot, entry, level)?;
        }
        Ok(None)
    }

    /// Frees the user half: drops this hierarchy's reference to every table
    /// and page it reaches, freeing those it held alone, then the root. A
    /// table still shared with another hierarchy is left to it whole.
    pub fn destroy_lower_half(self, mem: &mut impl PhysicalMemory, frames: &mut impl Frames) {
        fn walk<F: EntryFormat>(
            mem: &mut impl PhysicalMemory,
            frames: &mut impl Frames,
            table: u64,
            level: u32,
            entries: u64,
        ) {
            for i in 0..entries {
                let entry = mem.read_u64(table + i * 8);
                if !F::is_valid(entry) {
                    continue;
                }
                if F::is_leaf(entry, level) {
                    let size = match level {
                        0 => PageSize::Small,
                        1 => PageSize::Large,
                        _ => PageSize::Huge,
                    };
                    frames.free(F::address(entry) & !(size.bytes() - 1), size);
                } else {
                    let next = F::address(entry);
                    if !frames.is_shared(next) {
                        walk::<F>(mem, frames, next, level - 1, ENTRIES);
                    }
                    frames.free(next, PageSize::Small);
                }
            }
        }
        walk::<F>(mem, frames, self.root(), 3, ENTRIES / 2);
        frames.free(self.root(), PageSize::Small);
    }
}

/// Makes the table that `entry` (in `slot`, at `level`) points to private to
/// the hierarchy holding `slot`, and writable through it; returns the table.
/// A shared table is copied: both copies then point at everything below,
/// with one more reference each, and no writes through either (invariant).
#[inline]
fn private_table<F: EntryFormat>(
    mem: &mut impl PhysicalMemory,
    frames: &mut impl Frames,
    slot: u64,
    entry: u64,
    level: u32,
) -> Result<u64, MapError> {
    let next = F::address(entry);
    if F::table_writable(entry) {
        return Ok(next);
    }
    if !frames.is_shared(next) {
        mem.write_u64(slot, F::with_table_writable(entry, true));
        return Ok(next);
    }
    let copy = frames.alloc().ok_or(MapError::OutOfFrames)?;
    mem.copy(next, copy, FRAME_SIZE);
    for i in 0..ENTRIES {
        let e = mem.read_u64(next + i * 8);
        if !F::is_valid(e) {
            continue;
        }
        frames.share(F::address(e) & !(leaf_bytes::<F>(e, level - 1) - 1));
        let shared = if F::is_leaf(e, level - 1) {
            let f = F::flags(e);
            if f.is_writable() {
                F::with_cow(F::with_leaf_writable(e, false), true)
            } else {
                e
            }
        } else {
            F::with_table_writable(e, false)
        };
        mem.write_u64(next + i * 8, shared);
        mem.write_u64(copy + i * 8, shared);
    }
    mem.write_u64(
        slot,
        F::with_table_writable(F::with_address(entry, copy), true),
    );
    frames.free(next, PageSize::Small);
    Ok(copy)
}

/// Bytes mapped by entry `e` at `level` if it is a leaf, else one frame.
fn leaf_bytes<F: EntryFormat>(e: u64, level: u32) -> u64 {
    if F::is_leaf(e, level) {
        match level {
            0 => FRAME_SIZE,
            1 => crate::LARGE_PAGE_SIZE,
            _ => crate::paging::HUGE_PAGE_SIZE,
        }
    } else {
        FRAME_SIZE
    }
}

/// Copies the 2 MiB page at `page` into 512 private 4 KiB frames, when no
/// whole 2 MiB block is free, and returns a table mapping them with the
/// permissions of the large entry `writable` (design D8 of
/// `add-sockets-and-poll`). Gives everything back if frames run out.
fn copy_as_small_pages<F: EntryFormat>(
    mem: &mut impl PhysicalMemory,
    frames: &mut impl Frames,
    page: u64,
    writable: u64,
) -> Result<u64, MapError> {
    let table = frames.alloc().ok_or(MapError::OutOfFrames)?;
    let flags = F::flags(writable);
    for i in 0..ENTRIES {
        let Some(copy) = frames.alloc() else {
            for j in 0..i {
                frames.free(F::address(mem.read_u64(table + j * 8)), PageSize::Small);
            }
            frames.free(table, PageSize::Small);
            return Err(MapError::OutOfFrames);
        };
        mem.copy(page + i * FRAME_SIZE, copy, FRAME_SIZE);
        let leaf = F::with_cow(F::leaf(copy, PageSize::Small, flags), false);
        mem.write_u64(table + i * 8, leaf);
    }
    Ok(table)
}

#[cfg(test)]
mod tests;
