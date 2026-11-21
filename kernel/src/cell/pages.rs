//! The page cache: file pages shared by content between the cells that
//! map them.

use super::*;

/// Most file pages kept (design D1 of `add-dynamic-linking`): 256 MiB, as
/// much as a cell may hold in files (Node.js maps over 100 MiB of its own;
/// change `add-node-workload`).
const KEPT_PAGES: usize = 65536;

/// Pages of files mapped into cells, kept by content (spec `cells`, file
/// pages shared between cells): one read-only frame for every mapping of the
/// same bytes. Its frames are taken and given back like those of
/// [`IMAGES`].
pub(super) static PAGES: Global<PageCache> = Global::new(PageCache::new(KEPT_PAGES));

/// The frames of mapped file pages, from [`PAGES`]; a page written now is
/// made visible to instruction fetch.
pub(super) struct KernelPages;

impl<A: Frames> FilePages<DirectMap, A> for KernelPages {
    fn frame(&mut self, page: &rhizo_fs::Page, mem: &mut DirectMap, frames: &mut A) -> Option<u64> {
        if page.key().is_none() {
            // A hole or a page past the end is the cell's alone, never kept:
            // it comes from the cell's frames, counted in its memory quota
            // like anonymous memory (finding S20).
            let phys = frames.alloc()?;
            rhizo_hal::PhysicalMemory::zero_frame(mem, phys);
            return Some(phys);
        }
        // Short of frames, what is kept goes first (outside the lock).
        let share = || PAGES.with(|pages| pages.share(page, &mut DirectMap, &mut ImageFrames));
        let shared = match share() {
            Share::OutOfMemory if drop_images() => share(),
            shared => shared,
        };
        let (phys, fresh) = match shared {
            Share::Kept(phys, fresh) => (phys, fresh),
            // Not kept (no room, or a count that is full): a copy from the
            // cell's frames, counted in its quota.
            Share::Own => (own_copy(page, mem, frames)?, true),
            Share::OutOfMemory => return None,
        };
        if fresh {
            <Self as FilePages<DirectMap, A>>::code(self, phys);
        }
        Some(phys)
    }

    fn code(&mut self, phys: u64) {
        user::sync_code(DirectMap::virt(phys), FRAME_SIZE);
    }
}

/// File pages kept now.
#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests")
)]
pub fn kept_pages() -> usize {
    PAGES.with(|pages| pages.len())
}
