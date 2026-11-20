//! Copy-on-write that shares page tables as well as pages (design D2 of
//! `add-fork`).
//!
//! Invariant, kept by every operation here: an entry that points at a frame
//! with more than one reference (a table, or a page) forbids writes: a table
//! entry through its table-level permission, a leaf with the copy-on-write
//! mark. Forking therefore only has to mark the parent's top-level entries,
//! and a write fault copies only what it must, on the path to the page.

use crate::FRAME_SIZE;
use crate::paging::{EntryFormat, MapError, PageSize, PageTable};
use rhizo_hal::PhysicalMemory;

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

    /// Resolves a write to `virt` refused only because of sharing: copies the
    /// shared tables on the path and the page if it still has other
    /// references, or makes it writable again if not. Returns `false` if the
    /// write is refused for a real reason (no page, or a read-only one). The
    /// caller flushes the TLB entry for `virt`.
    pub fn resolve_write(
        &self,
        mem: &mut impl PhysicalMemory,
        frames: &mut impl Frames,
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
            match copy {
                Some(copy) => {
                    copy_frames(mem, page, copy, size.bytes());
                    mem.write_u64(slot, F::with_address(writable, copy));
                }
                None if size == PageSize::Large => {
                    let table = copy_as_small_pages::<F>(mem, frames, page, writable)?;
                    mem.write_u64(slot, F::table(table));
                }
                None => return Err(MapError::OutOfFrames),
            }
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
            let next = F::address(entry);
            if F::table_writable(entry) {
                table = next;
                continue;
            }
            table = if frames.is_shared(next) {
                let copy = frames.alloc().ok_or(MapError::OutOfFrames)?;
                copy_frames(mem, next, copy, FRAME_SIZE);
                // Both copies now point at everything below: one more
                // reference each, and no writes through either (invariant).
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
                copy
            } else {
                mem.write_u64(slot, F::with_table_writable(entry, true));
                next
            };
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
        copy_frames(mem, page + i * FRAME_SIZE, copy, FRAME_SIZE);
        let leaf = F::with_cow(F::leaf(copy, PageSize::Small, flags), false);
        mem.write_u64(table + i * 8, leaf);
    }
    Ok(table)
}

fn copy_frames(mem: &mut impl PhysicalMemory, from: u64, to: u64, bytes: u64) {
    let mut buf = [0u8; 512];
    for off in (0..bytes).step_by(buf.len()) {
        mem.read_bytes(from + off, &mut buf);
        mem.write_bytes(to + off, &buf);
    }
}

#[cfg(test)]
mod tests;
