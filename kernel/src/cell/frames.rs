//! Frames as cells see them: the kernel's allocator with reference
//! counts for frames that forks share, charged to a cell's quota.

use super::*;

/// The kernel's frame allocator, as cells see it.
#[derive(Debug)]
pub(super) struct KernelFrames;

/// Reference counts of frames, and which frames the page cache keeps.
#[derive(Debug)]
pub(super) struct Refs {
    /// Extra references per frame (design D1 of `add-fork`): index = frame
    /// number, value = references beyond the first. Grown on demand, so
    /// cells that never fork cost nothing.
    extra: Vec<u16>,
    /// One bit per frame the page cache keeps, until it is freed: a
    /// reference to one that a cell gives back credits it nothing (finding
    /// S30 in `docs/security.md`). Grown on demand.
    kept: Vec<u64>,
}

impl Refs {
    #[inline(always)]
    fn is_kept(&self, index: usize) -> bool {
        self.kept
            .get(index / 64)
            .is_some_and(|w| w >> (index % 64) & 1 == 1)
    }

    fn set_kept(&mut self, index: usize, kept: bool) {
        let (word, bit) = (index / 64, 1u64 << (index % 64));
        if kept {
            if self.kept.len() <= word {
                self.kept.resize(word + 1, 0);
            }
            self.kept[word] |= bit;
        } else if let Some(w) = self.kept.get_mut(word) {
            *w &= !bit;
        }
    }
}

pub(super) static SHARED: Global<Refs> = Global::new(Refs {
    extra: Vec::new(),
    kept: Vec::new(),
});

/// Makes the bitmap of kept frames at boot: what it grows to later is
/// the same allocation, so no test sees it appear as a leak.
pub(super) fn init_refs() {
    SHARED.with(|s| s.kept.reserve(1));
}

/// Marks frame `phys` as one the page cache keeps (see [`Refs::kept`]).
pub(super) fn keep(phys: u64) {
    SHARED.with(|s| s.set_kept((phys / FRAME_SIZE) as usize, true));
}

impl Frames for KernelFrames {
    /// Short of frames, the images kept loaded go first (design D4 of
    /// `add-image-cache`).
    fn alloc(&mut self) -> Option<u64> {
        ImageFrames
            .alloc()
            .or_else(|| drop_images().then(|| ImageFrames.alloc()).flatten())
    }

    /// Without dropping images: whoever asks for 2 MiB falls back to
    /// 4 KiB pages, which may.
    fn alloc_large(&mut self) -> Option<u64> {
        ImageFrames.alloc_large()
    }

    /// What the kernel keeps for itself only once nothing else is left.
    fn alloc_set_aside(&mut self) -> Option<u64> {
        self.alloc().or_else(|| memory::alloc_set_aside(1))
    }

    fn alloc_large_set_aside(&mut self) -> Option<u64> {
        self.alloc_large()
            .or_else(|| memory::alloc_set_aside((LARGE_PAGE_SIZE / FRAME_SIZE) as usize))
    }

    fn free(&mut self, phys: u64, size: PageSize) {
        drop_ref(phys, size, false, false);
    }

    /// A reference to a frame the page cache keeps credits nothing (the
    /// cache holds another until it drops the frame, see [`unkeep`]).
    fn release(&mut self, phys: u64, size: PageSize) -> bool {
        drop_ref(phys, size, false, true)
    }

    fn share(&mut self, phys: u64) {
        let _p = cost::scope(Part::Frames);
        let index = (phys / FRAME_SIZE) as usize;
        SHARED.with(|s| {
            let s = &mut s.extra;
            if s.len() <= index {
                s.resize(index + 1, 0);
            }
            s[index] = s[index]
                .checked_add(1)
                .expect("more than 65535 cells share a frame");
        });
    }

    fn is_shared(&self, phys: u64) -> bool {
        let _p = cost::scope(Part::Frames);
        SHARED.with(|s| {
            s.extra
                .get((phys / FRAME_SIZE) as usize)
                .is_some_and(|&e| e > 0)
        })
    }

    fn try_share(&mut self, phys: u64) -> bool {
        let _p = cost::scope(Part::Frames);
        let index = (phys / FRAME_SIZE) as usize;
        SHARED.with(|s| {
            let s = &mut s.extra;
            if s.len() <= index {
                s.resize(index + 1, 0);
            }
            s[index].checked_add(1).map(|n| s[index] = n).is_some()
        })
    }
}

/// Drops one reference to the frame at `phys`, after marking it kept no
/// more if `unkeep`, and frees it with the last; returns, if `ask`,
/// whether the reference credits whoever gave it back (see
/// [`Frames::release`]). Without `ask` (a cell's teardown, whose meter
/// goes) nothing is looked up for it.
#[inline(always)]
fn drop_ref(phys: u64, size: PageSize, unkeep: bool, ask: bool) -> bool {
    let _p = cost::scope(Part::Frames);
    let index = (phys / FRAME_SIZE) as usize;
    let (shared, credit) = SHARED.with(|s| {
        if unkeep {
            s.set_kept(index, false);
        }
        match s.extra.get_mut(index) {
            Some(extra) if *extra > 0 => {
                *extra -= 1;
                (true, !ask || !s.is_kept(index))
            }
            _ => (false, true),
        }
    });
    if !shared {
        let frames = (size.bytes() / FRAME_SIZE) as usize;
        memory::with_frames(|f| f.free_contiguous(phys, frames));
    }
    credit
}

/// Drops the page cache's (or an image's) reference to the frame at
/// `phys`: the frame is kept by the kernel no more, so the references
/// cells still hold credit them when given back.
pub(super) fn unkeep(phys: u64, size: PageSize) {
    drop_ref(phys, size, true, false);
}

/// Whether the cell of `meter` may take `bytes` more: what its memory
/// limit leaves is set aside for it (design D4 of `add-cell-hierarchy`),
/// only the rest must be free for cells.
pub(super) fn may_take(meter: &Meter, bytes: u64) -> bool {
    let room = match meter.limit() < Quota::NONE.frames() {
        true => meter
            .limit()
            .saturating_sub(meter.used())
            .saturating_mul(FRAME_SIZE),
        false => 0,
    };
    bytes <= room || cells_may_take(bytes - room)
}

/// The frames of a cell, counted by its meter against its memory quota
/// (design D1 of `add-cell-quotas`).
pub(super) fn cell_frames(meter: &Meter) -> Metered<'_, KernelFrames> {
    Metered {
        frames: KernelFrames,
        meter,
    }
}

/// Held while page tables and frames that cells may share change: a fork's
/// copy-on-write tables are resolved, unmapped or freed by one CPU at a
/// time (design D6 of `add-smp-scheduling`). Cells that share nothing never
/// take it.
pub(super) static SHARING: Global<()> = Global::new(());

/// Runs `f`, under [`SHARING`] when `live` may share frames with another
/// cell.
pub(super) fn sharing<R>(shares: bool, f: impl FnOnce() -> R) -> R {
    if shares { SHARING.with(|()| f()) } else { f() }
}

/// Drops a snapshot's reference to a frame it shares (design D1 of
/// `add-shared-snapshots`). Only a count goes down, so no lock of shared
/// tables is needed: a cell that sees the frame no longer shared owns it.
pub(super) fn release_frame(phys: u64, size: PageSize) {
    KernelFrames.free(phys, size);
}
