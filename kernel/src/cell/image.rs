//! Executables kept loaded (change `add-image-cache`): the page tables
//! of each loaded image, copied for every cell spawned from it.

use super::*;

/// Executables kept loaded (design D3 of `add-image-cache`).
const KEPT_IMAGES: usize = 4;

/// Executables loaded once and copied by every later spawn of them (spec
/// `cells`, design D1 of `add-image-cache`): each a hierarchy holding only
/// its segments, never run, whose tables and pages are only read once it is
/// here. Its frames are the kernel's, taken like a cell's (never from the
/// reserve) and given back before any cell is refused memory (D4).
static IMAGES: Global<Images<KEPT_IMAGES>> = Global::new(Images::new());

/// Drops every image kept loaded and every file page kept whose dropping
/// frees memory (design D3 of `close-cross-cell-leaks`); returns whether
/// there was one. Pages that cells map stay kept.
#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "also used by in-kernel tests")
)]
pub fn drop_images() -> bool {
    let any = IMAGES.with(|images| {
        let mut any = false;
        while let Some(root) = images.pop() {
            space::destroy(
                PageTable::<Format>::from_root(root),
                &mut DirectMap,
                &mut ImageFrames,
            );
            any = true;
        }
        any
    });
    PAGES.with(|pages| pages.trim(&mut ImageFrames)) || any
}

/// The read-only pages of the executables kept loaded, sorted: frames of
/// public objects, which copies into other domains share (design D21 of
/// `add-cell-hierarchy`). Gathered on demand, by the rare operations that
/// need them.
pub(super) fn image_frames() -> Vec<u64> {
    let mut frames = Vec::new();
    IMAGES.with(|images| {
        for root in images.roots() {
            PageTable::<Format>::from_root(root).for_each_lower_leaf(
                &DirectMap,
                &mut |_, phys, _, flags| {
                    if !flags.is_writable() && !flags.contains(PageFlags::COPY_ON_WRITE) {
                        frames.push(phys);
                    }
                },
            );
        }
    });
    frames.sort_unstable();
    frames
}

/// Whether cells may take `bytes` more, dropping the images kept loaded if
/// that is what it takes (design D4 of `add-image-cache`).
pub(crate) fn cells_may_take(bytes: u64) -> bool {
    memory::cells_may_take(bytes) || (drop_images() && memory::cells_may_take(bytes))
}

/// A space of its own holding the segments of the executable `data`, parsed
/// as `image` (design D2 of `add-image-cache`): a copy of the space kept
/// for it, loaded first if it is not kept yet.
pub(super) fn image_space(
    data: &'static [u8],
    image: &Image,
) -> Result<PageTable<Format>, ElfError> {
    let key = (data.as_ptr() as u64, data.len() as u64);
    if let Some(pt) = IMAGES.with(|images| copy_image(images, key, image)) {
        return Ok(pt);
    }
    // Loaded outside the lock: loading may drop the images kept.
    let loaded = rhizo_cell::load::<Format>(image, &mut DirectMap, &mut KernelFrames)?;
    sync_code(&loaded, image);
    IMAGES
        .with(|images| {
            if images.get(key).is_some() {
                // Another CPU loaded it meanwhile.
                space::destroy(loaded, &mut DirectMap, &mut ImageFrames);
            } else if let Some(old) = images.insert(key, loaded.root()) {
                space::destroy(
                    PageTable::<Format>::from_root(old),
                    &mut DirectMap,
                    &mut ImageFrames,
                );
            }
            copy_image(images, key, image)
        })
        .ok_or(ElfError::OutOfMemory)
}

/// A copy of the space kept for `key`: its read-only pages shared, its
/// tables and writable pages the copy's own. Short of frames for that, the
/// kept space itself, taken out, with every other image dropped: a spawn
/// never needs more memory than a load. `None` if `key` is not kept.
fn copy_image(
    images: &mut Images<KEPT_IMAGES>,
    key: images::Key,
    image: &Image,
) -> Option<PageTable<Format>> {
    let kept = PageTable::<Format>::from_root(images.get(key)?);
    if let Ok(copy) = kept.clone_lower_half(&mut DirectMap, &mut ImageFrames, image.span()) {
        return Some(copy);
    }
    images.remove(key);
    while let Some(root) = images.pop() {
        space::destroy(
            PageTable::<Format>::from_root(root),
            &mut DirectMap,
            &mut ImageFrames,
        );
    }
    Some(kept)
}

/// The frames of [`KernelFrames`] for work under the [`IMAGES`] lock: an
/// allocation that fails does not try to drop images.
#[derive(Debug)]
pub(super) struct ImageFrames;

impl Frames for ImageFrames {
    fn alloc(&mut self) -> Option<u64> {
        let _p = cost::scope(Part::Frames);
        memory::alloc_for_cell(1)
    }

    fn alloc_large(&mut self) -> Option<u64> {
        let _p = cost::scope(Part::Frames);
        memory::alloc_for_cell((LARGE_PAGE_SIZE / FRAME_SIZE) as usize)
    }

    fn free(&mut self, phys: u64, size: PageSize) {
        unkeep(phys, size);
    }

    fn share(&mut self, phys: u64) {
        KernelFrames.share(phys);
    }

    fn is_shared(&self, phys: u64) -> bool {
        KernelFrames.is_shared(phys)
    }

    fn try_share(&mut self, phys: u64) -> bool {
        KernelFrames.try_share(phys)
    }
}

/// Makes the executable pages just written through the direct map visible to
/// instruction fetch (needed on AArch64; nothing to do on x86_64). Once per
/// loaded image: its copies share these pages, never written again (design
/// D5 of `add-image-cache`).
fn sync_code(pt: &PageTable<Format>, image: &Image) {
    for s in image.segments().filter(|s| s.flags.is_executable()) {
        let first = s.vaddr & !(FRAME_SIZE - 1);
        for page in (first..s.vaddr + s.mem_size).step_by(FRAME_SIZE as usize) {
            if let Some(t) = pt.translate(&DirectMap, page) {
                user::sync_code(DirectMap::virt(t.phys), FRAME_SIZE);
            }
        }
    }
}
