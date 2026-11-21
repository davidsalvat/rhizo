//! The page cache: file pages shared by content between the cells of one
//! domain that map them, and public pages shared by every domain (design
//! D21 of `add-cell-hierarchy`).

use super::*;

/// Most file pages kept per domain (design D1 of `add-dynamic-linking`):
/// 256 MiB, as much as a cell may hold in files (Node.js maps over 100 MiB
/// of its own; change `add-node-workload`).
const KEPT_PAGES: usize = 65536;

/// Pages of files mapped into cells, kept by domain and content (spec
/// `cells`, file pages shared between cells; spec `isolation`, file pages
/// kept per domain): one read-only frame for every mapping of the same
/// bytes in a domain, and one for every mapping of a public page. Its
/// frames are taken and given back like those of [`IMAGES`].
pub(super) static PAGES: Global<PageCache> = Global::new(PageCache::new(KEPT_PAGES));

/// The frames of mapped file pages, from [`PAGES`]; a page written now is
/// made visible to instruction fetch.
pub(super) struct KernelPages;

impl<A: Frames> FilePages<DirectMap, A> for KernelPages {
    fn frame(
        &mut self,
        page: &rhizo_fs::Page,
        scope: u32,
        mem: &mut DirectMap,
        frames: &mut A,
    ) -> Option<u64> {
        if page.key().is_none() {
            // A hole or a page past the end is the cell's alone, never kept:
            // it comes from the cell's frames, counted in its memory quota
            // like anonymous memory (finding S20).
            let phys = frames.alloc()?;
            rhizo_hal::PhysicalMemory::zero_frame(mem, phys);
            return Some(phys);
        }
        // Short of frames, what is kept goes first (outside the lock).
        let share =
            || PAGES.with(|pages| pages.share(page, scope, &mut DirectMap, &mut ImageFrames));
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

/// Publishes the files of `fs` (design D21 of `add-cell-hierarchy`): makes
/// them public, then fills and pins a public frame for every page of their
/// executables' and libraries' loadable segments, so that a map of one by
/// any domain always hits. Returns the pages pinned, for
/// [`withdraw_pages`]; a page past what memory holds is not published (the
/// domains that map it keep their own).
pub(super) fn publish_files(fs: &mut Fs) -> Vec<rhizo_fs::Page> {
    fs.publish();
    let mut pinned = Vec::new();
    for (ino, size) in fs.files() {
        let Ok(head) = fs.page(ino, 0) else {
            continue;
        };
        let Ok(image) = Image::parse_file(head.bytes(), size, user::MACHINE) else {
            continue; // not an executable or a library of this machine
        };
        for s in image.segments() {
            let first = s.file_offset / FRAME_SIZE;
            let end = (s.file_offset + s.file_size).div_ceil(FRAME_SIZE);
            for index in first..end {
                let Ok(page) = fs.page(ino, index) else {
                    continue;
                };
                let published =
                    PAGES.with(|pages| pages.publish(&page, &mut DirectMap, &mut ImageFrames));
                let Some((phys, fresh)) = published else {
                    continue;
                };
                if fresh {
                    user::sync_code(DirectMap::virt(phys), FRAME_SIZE);
                }
                pinned.push(page);
            }
        }
    }
    pinned
}

/// Ends the publication of `pages` ([`publish_files`]): each goes once
/// nothing maps it and no other publication pins it.
pub(super) fn withdraw_pages(pages: &[rhizo_fs::Page]) {
    PAGES.with(|cache| pages.iter().for_each(|page| cache.withdraw(page)));
}

/// Tells whether a frame is of a public object, which every domain may
/// share (design D21 of `add-cell-hierarchy`): a published page of a file,
/// or a read-only page of an executable the kernel keeps loaded (as they
/// are now).
pub(super) fn public_frames() -> impl FnMut(u64) -> bool {
    let images = image_frames();
    move |phys| images.binary_search(&phys).is_ok() || PAGES.with(|p| p.is_public(phys))
}

/// Tells whether a frame is one a cache of the kernel holds (a file page
/// kept for any domain, or a page of an executable kept loaded, as they
/// are now): one whose count never tells a cell another cell's end.
pub(super) fn kept_frames() -> impl FnMut(u64) -> bool {
    let images = image_frames();
    move |phys| images.binary_search(&phys).is_ok() || PAGES.with(|p| p.holds(phys))
}

/// File pages kept now.
#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests")
)]
pub fn kept_pages() -> usize {
    PAGES.with(|pages| pages.len())
}

/// File pages kept for domain `domain` ([`rhizo_abi::PUBLIC`]: public
/// pages).
#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests")
)]
pub fn kept_pages_of(domain: u32) -> usize {
    PAGES.with(|pages| pages.kept(domain))
}

/// Keeps at most `pages` file pages per domain from now on (what an
/// in-kernel test can fill), or [`KEPT_PAGES`] again with `None`.
#[cfg(feature = "ktest")]
pub fn limit_kept_pages(pages: Option<usize>) {
    PAGES.with(|cache| cache.set_limit(pages.unwrap_or(KEPT_PAGES)));
}
