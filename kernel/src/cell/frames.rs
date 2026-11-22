//! Frames as cells see them: the kernel's allocator with reference
//! counts for frames that forks share, charged to a cell's quota.

use super::*;

/// The kernel's frame allocator, as cells see it.
#[derive(Debug)]
pub(super) struct KernelFrames;

/// Extra references per frame (design D1 of `add-fork`): index = frame
/// number, value = references beyond the first. Grown on demand, so cells
/// that never fork cost nothing.
pub(super) static SHARED: Global<Vec<u16>> = Global::new(Vec::new());

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

    fn free(&mut self, phys: u64, size: PageSize) {
        let _p = cost::scope(Part::Frames);
        let index = (phys / FRAME_SIZE) as usize;
        let shared = SHARED.with(|s| match s.get_mut(index) {
            Some(extra) if *extra > 0 => {
                *extra -= 1;
                true
            }
            _ => false,
        });
        if !shared {
            let frames = (size.bytes() / FRAME_SIZE) as usize;
            memory::with_frames(|f| f.free_contiguous(phys, frames));
        }
    }

    fn share(&mut self, phys: u64) {
        let _p = cost::scope(Part::Frames);
        let index = (phys / FRAME_SIZE) as usize;
        SHARED.with(|s| {
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
        SHARED.with(|s| s.get((phys / FRAME_SIZE) as usize).is_some_and(|&e| e > 0))
    }

    fn try_share(&mut self, phys: u64) -> bool {
        let _p = cost::scope(Part::Frames);
        let index = (phys / FRAME_SIZE) as usize;
        SHARED.with(|s| {
            if s.len() <= index {
                s.resize(index + 1, 0);
            }
            s[index].checked_add(1).map(|n| s[index] = n).is_some()
        })
    }
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
