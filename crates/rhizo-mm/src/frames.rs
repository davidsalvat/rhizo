//! Physical frame allocator: one bit per 4 KiB frame, next-fit search.
//!
//! A set bit means the frame is **free**, so an all-zero bitmap (for example
//! one in `.bss`) already means "everything in use" and costs nothing to
//! initialize. Callers mark RAM regions free
//! ([`FrameAllocator::add_free_region`]) and re-reserve what must never be
//! handed out ([`FrameAllocator::reserve`]). Range operations work on whole
//! 64-bit words. The search order depends only on the calls made, so
//! allocation is deterministic.

use crate::{FRAME_SIZE, align_down, align_up};

const BITS: usize = 64;

/// Bitmap allocator for 4 KiB physical frames.
#[derive(Debug)]
pub struct FrameAllocator<'a> {
    bitmap: &'a mut [u64],
    /// Number of frames covered by the bitmap (frame `i` is at `i * 4096`).
    frames: usize,
    /// Frames made available by `add_free_region` (net of reservations).
    usable: usize,
    free: usize,
    /// Next-fit hint: frame index where the next search starts.
    hint: usize,
}

impl<'a> FrameAllocator<'a> {
    /// Creates an allocator for `frames` frames with every frame in use.
    ///
    /// # Panics
    /// If `bitmap` holds fewer than `frames` bits.
    pub fn new(bitmap: &'a mut [u64], frames: usize) -> Self {
        bitmap.fill(0);
        Self::from_zeroed(bitmap, frames)
    }

    /// Like [`new`](Self::new), but skips clearing `bitmap`, which must
    /// already be all zeros (for example, a static in `.bss`).
    ///
    /// # Panics
    /// If `bitmap` holds fewer than `frames` bits.
    pub fn from_zeroed(bitmap: &'a mut [u64], frames: usize) -> Self {
        assert!(bitmap.len() * BITS >= frames, "bitmap too small");
        Self {
            bitmap,
            frames,
            usable: 0,
            free: 0,
            hint: 0,
        }
    }

    /// Marks the frames fully inside `[start, end)` as free.
    pub fn add_free_region(&mut self, start: u64, end: u64) {
        let first = (align_up(start, FRAME_SIZE) / FRAME_SIZE) as usize;
        let last = ((align_down(end, FRAME_SIZE) / FRAME_SIZE) as usize).min(self.frames);
        let changed = self.update_range(first, last, true);
        self.usable += changed;
        self.free += changed;
    }

    /// Marks every frame touching `[start, end)` as permanently in use.
    pub fn reserve(&mut self, start: u64, end: u64) {
        let first = (align_down(start, FRAME_SIZE) / FRAME_SIZE) as usize;
        let last = ((align_up(end, FRAME_SIZE) / FRAME_SIZE) as usize).min(self.frames);
        let changed = self.update_range(first, last, false);
        self.usable -= changed;
        self.free -= changed;
    }

    /// Allocates one frame and returns its physical address.
    pub fn alloc(&mut self) -> Option<u64> {
        if self.free == 0 {
            return None;
        }
        let start = self.hint.min(self.frames) / BITS;
        let words = self.bitmap.len();
        for i in (start..words).chain(0..start) {
            let word = self.bitmap[i];
            if word != 0 {
                let frame = i * BITS + word.trailing_zeros() as usize;
                if frame < self.frames {
                    self.bitmap[i] = word & (word - 1);
                    self.free -= 1;
                    self.hint = frame + 1;
                    return Some(frame as u64 * FRAME_SIZE);
                }
            }
        }
        None
    }

    /// Allocates `count` physically contiguous frames and returns the address
    /// of the first one. Returns `None` if no free run of that length exists.
    pub fn alloc_contiguous(&mut self, count: usize) -> Option<u64> {
        if count == 1 {
            return self.alloc();
        }
        if count == 0 || count > self.free {
            return None;
        }
        let start = self
            .find_run(self.hint.min(self.frames), self.frames, count)
            .or_else(|| self.find_run(0, (self.hint + count - 1).min(self.frames), count))?;
        self.update_range(start, start + count, false);
        self.free -= count;
        self.hint = start + count;
        Some(start as u64 * FRAME_SIZE)
    }

    /// Frees one frame previously returned by [`alloc`](Self::alloc).
    ///
    /// # Panics
    /// If `addr` is unaligned, outside the managed range, or not allocated.
    pub fn free(&mut self, addr: u64) {
        self.free_contiguous(addr, 1);
    }

    /// Frees `count` contiguous frames starting at `addr`.
    ///
    /// # Panics
    /// Same conditions as [`free`](Self::free), for any frame of the run.
    pub fn free_contiguous(&mut self, addr: u64, count: usize) {
        assert!(
            addr.is_multiple_of(FRAME_SIZE),
            "freeing unaligned frame {addr:#x}"
        );
        let first = (addr / FRAME_SIZE) as usize;
        assert!(
            first
                .checked_add(count)
                .is_some_and(|end| end <= self.frames),
            "freeing frame {addr:#x} outside managed memory"
        );
        if let Some(frame) = (first..first + count).find(|&f| self.is_free(f)) {
            panic!("double free of frame {:#x}", frame as u64 * FRAME_SIZE);
        }
        self.update_range(first, first + count, true);
        self.free += count;
    }

    /// Returns whether the frame at `addr` is currently in use (or reserved).
    pub fn is_allocated(&self, addr: u64) -> bool {
        let frame = (addr / FRAME_SIZE) as usize;
        frame >= self.frames || !self.is_free(frame)
    }

    /// Number of frames that were made available for allocation.
    pub fn usable_frames(&self) -> usize {
        self.usable
    }

    /// Number of frames currently free.
    pub fn free_frames(&self) -> usize {
        self.free
    }

    /// Sets (`free = true`) or clears the bits of frames `[first, last)`,
    /// a word at a time. Returns how many bits changed.
    fn update_range(&mut self, first: usize, last: usize, free: bool) -> usize {
        let mut changed = 0;
        let mut frame = first;
        while frame < last {
            let bit = frame % BITS;
            let n = (BITS - bit).min(last - frame);
            let mask = if n == BITS {
                u64::MAX
            } else {
                ((1u64 << n) - 1) << bit
            };
            let word = &mut self.bitmap[frame / BITS];
            let before = *word;
            *word = if free { before | mask } else { before & !mask };
            changed += (before ^ *word).count_ones() as usize;
            frame += n;
        }
        changed
    }

    fn find_run(&self, from: usize, to: usize, count: usize) -> Option<usize> {
        let mut frame = from;
        while frame + count <= to {
            if frame.is_multiple_of(BITS) && self.bitmap[frame / BITS] == 0 {
                frame += BITS;
                continue;
            }
            if !self.is_free(frame) {
                frame += 1;
                continue;
            }
            match (frame..frame + count).find(|&f| !self.is_free(f)) {
                None => return Some(frame),
                Some(used) => frame = used + 1,
            }
        }
        None
    }

    fn is_free(&self, frame: usize) -> bool {
        self.bitmap[frame / BITS] & (1 << (frame % BITS)) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::HashSet;

    const MIB: u64 = 1024 * 1024;

    fn allocator(bitmap: &mut [u64], frames: usize) -> FrameAllocator<'_> {
        FrameAllocator::new(bitmap, frames)
    }

    #[test]
    fn only_free_regions_are_allocated() {
        let mut bits = [0u64; 64];
        let mut fa = allocator(&mut bits, 4096); // 16 MiB
        fa.add_free_region(MIB, 2 * MIB);
        fa.add_free_region(4 * MIB, 4 * MIB + 3 * FRAME_SIZE);
        assert_eq!(fa.usable_frames(), 256 + 3);

        let mut seen = HashSet::new();
        while let Some(addr) = fa.alloc() {
            assert_eq!(addr % FRAME_SIZE, 0);
            assert!(
                (MIB..2 * MIB).contains(&addr)
                    || (4 * MIB..4 * MIB + 3 * FRAME_SIZE).contains(&addr)
            );
            assert!(seen.insert(addr), "frame {addr:#x} returned twice");
        }
        assert_eq!(seen.len(), 259);
        assert_eq!(fa.free_frames(), 0);
    }

    #[test]
    fn partial_frames_are_not_usable_and_reservations_round_outward() {
        let mut bits = [0u64; 1];
        let mut fa = allocator(&mut bits, 64);
        fa.add_free_region(100, 5 * FRAME_SIZE + 10); // frames 1..5
        assert_eq!(fa.usable_frames(), 4);
        fa.reserve(2 * FRAME_SIZE + 1, 3 * FRAME_SIZE + 1); // frames 2 and 3
        assert_eq!(fa.usable_frames(), 2);
        let a = fa.alloc().unwrap();
        let b = fa.alloc().unwrap();
        assert_eq!((a, b), (FRAME_SIZE, 4 * FRAME_SIZE));
        assert_eq!(fa.alloc(), None);
    }

    #[test]
    fn freed_frame_is_reused() {
        let mut bits = [0u64; 1];
        let mut fa = allocator(&mut bits, 64);
        fa.add_free_region(0, 4 * FRAME_SIZE);
        let all: Vec<_> = std::iter::from_fn(|| fa.alloc()).collect();
        assert_eq!(all.len(), 4);
        fa.free(all[2]);
        assert_eq!(fa.alloc(), Some(all[2]));
    }

    #[test]
    fn contiguous_runs() {
        let mut bits = [0u64; 2];
        let mut fa = allocator(&mut bits, 128);
        fa.add_free_region(0, 128 * FRAME_SIZE);
        fa.reserve(3 * FRAME_SIZE, 4 * FRAME_SIZE); // hole at frame 3
        let run = fa.alloc_contiguous(4).unwrap();
        assert_eq!(run, 4 * FRAME_SIZE, "run must skip the hole");
        for i in 0..4 {
            assert!(fa.is_allocated(run + i * FRAME_SIZE));
        }
        assert_eq!(fa.alloc_contiguous(200), None);
        fa.free_contiguous(run, 4);
        assert!(!fa.is_allocated(run));
    }

    #[test]
    #[should_panic(expected = "double free of frame 0x1000")]
    fn double_free_panics() {
        let mut bits = [0u64; 1];
        let mut fa = allocator(&mut bits, 64);
        fa.add_free_region(FRAME_SIZE, 2 * FRAME_SIZE);
        let f = fa.alloc().unwrap();
        fa.free(f);
        fa.free(f);
    }

    #[test]
    #[should_panic(expected = "unaligned")]
    fn unaligned_free_panics() {
        let mut bits = [0u64; 1];
        let mut fa = allocator(&mut bits, 64);
        fa.free(123);
    }

    #[test]
    #[should_panic(expected = "outside managed memory")]
    fn out_of_range_free_panics() {
        let mut bits = [0u64; 1];
        let mut fa = allocator(&mut bits, 64);
        fa.free(64 * FRAME_SIZE);
    }

    #[test]
    fn reserved_frames_are_never_freed_back_into_use() {
        let mut bits = [0u64; 1];
        let mut fa = allocator(&mut bits, 64);
        fa.add_free_region(0, 64 * FRAME_SIZE);
        fa.reserve(0, 16 * FRAME_SIZE);
        let got: Vec<_> = std::iter::from_fn(|| fa.alloc()).collect();
        assert!(got.iter().all(|&a| a >= 16 * FRAME_SIZE));
    }

    #[derive(Debug, Clone)]
    enum Op {
        Alloc(usize),
        Free(usize),
    }

    proptest! {
        #[test]
        fn random_sequences_keep_invariants(
            ops in proptest::collection::vec(
                prop_oneof![
                    (1usize..8).prop_map(Op::Alloc),
                    (0usize..64).prop_map(Op::Free),
                ],
                1..200,
            ),
            reserved_start in 0u64..64,
            reserved_len in 0u64..16,
        ) {
            let mut bits = [0u64; 4];
            let mut fa = FrameAllocator::new(&mut bits, 256);
            fa.add_free_region(0, 256 * FRAME_SIZE);
            let reserved = reserved_start..reserved_start + reserved_len;
            fa.reserve(reserved.start * FRAME_SIZE, reserved.end * FRAME_SIZE);

            let mut live: Vec<(u64, usize)> = Vec::new();
            let mut owned: HashSet<u64> = HashSet::new();
            for op in ops {
                match op {
                    Op::Alloc(n) => {
                        if let Some(addr) = fa.alloc_contiguous(n) {
                            prop_assert_eq!(addr % FRAME_SIZE, 0);
                            for i in 0..n as u64 {
                                let frame = addr + i * FRAME_SIZE;
                                prop_assert!(!reserved.contains(&(frame / FRAME_SIZE)));
                                prop_assert!(owned.insert(frame), "duplicate frame");
                            }
                            live.push((addr, n));
                        }
                    }
                    Op::Free(i) if !live.is_empty() => {
                        let (addr, n) = live.swap_remove(i % live.len());
                        fa.free_contiguous(addr, n);
                        for k in 0..n as u64 {
                            owned.remove(&(addr + k * FRAME_SIZE));
                        }
                    }
                    Op::Free(_) => {}
                }
                prop_assert_eq!(fa.free_frames() + owned.len(), fa.usable_frames());
            }
        }
    }
}
