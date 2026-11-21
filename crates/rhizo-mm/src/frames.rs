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
    /// Frame index where the next search starts: just past the last frame
    /// handed out, or the lowest frame freed since, so freed frames are
    /// reused before frames never handed out (design D1 of
    /// `reuse-freed-frames`).
    hint: usize,
    /// Bitmap words above this one hold no free frame (where
    /// [`alloc_high`](Self::alloc_high) starts looking).
    top: usize,
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
            top: 0,
        }
    }

    /// Marks the frames fully inside `[start, end)` as free.
    pub fn add_free_region(&mut self, start: u64, end: u64) {
        let first = (align_up(start, FRAME_SIZE) / FRAME_SIZE) as usize;
        let last = ((align_down(end, FRAME_SIZE) / FRAME_SIZE) as usize).min(self.frames);
        let changed = self.update_range(first, last, true);
        self.usable += changed;
        self.free += changed;
        if first < last {
            self.top = self.top.max((last - 1) / BITS);
        }
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

    /// Allocates the highest free frame. The kernel heap takes its pages
    /// here, so the pages it keeps cluster at the top of memory instead of
    /// splitting the free runs that large allocations need (design D8 of
    /// `add-smp-boot`).
    pub fn alloc_high(&mut self) -> Option<u64> {
        if self.free == 0 {
            return None;
        }
        for i in (0..=self.top.min(self.bitmap.len() - 1)).rev() {
            let mut word = self.bitmap[i];
            // Bits past the last managed frame are never set; skip them anyway.
            while word != 0 {
                let bit = BITS - 1 - word.leading_zeros() as usize;
                let frame = i * BITS + bit;
                if frame < self.frames {
                    self.bitmap[i] &= !(1 << bit);
                    self.free -= 1;
                    self.top = i;
                    return Some(frame as u64 * FRAME_SIZE);
                }
                word &= !(1 << bit);
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

    /// Allocates 512 contiguous frames aligned to 2 MiB (one large page) and
    /// returns the address of the first. Each candidate block is eight whole
    /// bitmap words, so the search compares words, not bits.
    pub fn alloc_large(&mut self) -> Option<u64> {
        const WORDS: usize = (crate::LARGE_PAGE_SIZE / FRAME_SIZE) as usize / BITS;
        let blocks = self.frames / (WORDS * BITS);
        if blocks == 0 || self.free < WORDS * BITS {
            return None;
        }
        let start = (self.hint / (WORDS * BITS)).min(blocks);
        let block = (start..blocks).chain(0..start).find(|&b| {
            self.bitmap[b * WORDS..(b + 1) * WORDS]
                .iter()
                .all(|&w| w == u64::MAX)
        })?;
        self.bitmap[block * WORDS..(block + 1) * WORDS].fill(0);
        self.free -= WORDS * BITS;
        self.hint = (block + 1) * WORDS * BITS;
        Some((block * WORDS * BITS) as u64 * FRAME_SIZE)
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
        if count == 1 {
            // Most frees: one bit.
            let (word, bit) = (first / BITS, 1u64 << (first % BITS));
            if self.bitmap[word] & bit != 0 {
                panic!("double free of frame {addr:#x}");
            }
            self.bitmap[word] |= bit;
        } else {
            // A word at a time (change `fast-frame-free`): a 2 MiB page is
            // checked and freed in eight steps, not 512.
            let free =
                words(first, first + count).find_map(|(i, mask)| match self.bitmap[i] & mask {
                    0 => None,
                    hit => Some(i * BITS + hit.trailing_zeros() as usize),
                });
            if let Some(frame) = free {
                panic!("double free of frame {:#x}", frame as u64 * FRAME_SIZE);
            }
            self.update_range(first, first + count, true);
        }
        self.free += count;
        self.hint = self.hint.min(first);
        self.top = self.top.max((first + count - 1) / BITS);
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
        for (i, mask) in words(first, last) {
            let word = &mut self.bitmap[i];
            let before = *word;
            *word = if free { before | mask } else { before & !mask };
            changed += (before ^ *word).count_ones() as usize;
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

/// The bitmap words holding frames `[first, last)`, each with the mask of
/// those frames' bits.
fn words(first: usize, last: usize) -> impl Iterator<Item = (usize, u64)> {
    let mut frame = first;
    core::iter::from_fn(move || {
        if frame >= last {
            return None;
        }
        let bit = frame % BITS;
        let n = (BITS - bit).min(last - frame);
        let mask = if n == BITS {
            u64::MAX
        } else {
            ((1u64 << n) - 1) << bit
        };
        let word = frame / BITS;
        frame += n;
        Some((word, mask))
    })
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
    fn high_frames_come_from_the_top_and_leave_runs_whole() {
        let mut bits = [0u64; 4];
        let mut fa = allocator(&mut bits, 200);
        fa.add_free_region(FRAME_SIZE, 190 * FRAME_SIZE); // frames 1..190
        assert_eq!(fa.alloc_high(), Some(189 * FRAME_SIZE));
        assert_eq!(fa.alloc_high(), Some(188 * FRAME_SIZE));
        let low = fa.alloc().unwrap();
        assert_eq!(low, FRAME_SIZE, "single frames still come from the bottom");
        fa.free(189 * FRAME_SIZE);
        assert_eq!(fa.alloc_high(), Some(189 * FRAME_SIZE), "freed top reused");
        // Everything between stays one run.
        assert_eq!(fa.alloc_contiguous(186), Some(2 * FRAME_SIZE));
        assert_eq!(fa.alloc_high(), None);
        assert_eq!(fa.free_frames(), 0);
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
    fn large_blocks_are_aligned_whole_and_freed_as_runs() {
        let mut bits = [0u64; 64]; // 16 MiB
        let mut fa = allocator(&mut bits, 4096);
        fa.add_free_region(MIB, 7 * MIB); // whole 2 MiB blocks: [2, 4) and [4, 6)
        fa.reserve(4 * MIB + 5 * FRAME_SIZE, 4 * MIB + 6 * FRAME_SIZE);
        let free = fa.free_frames();
        let a = fa.alloc_large().unwrap();
        assert_eq!(a, 2 * MIB, "[4, 6) has a reserved frame");
        assert_eq!(fa.free_frames(), free - 512);
        assert_eq!(fa.alloc_large(), None);
        // Single frames still come from the partial blocks around it.
        let one = fa.alloc().unwrap();
        assert!(!(2 * MIB..4 * MIB).contains(&one));
        fa.free_contiguous(a, 512);
        assert_eq!(fa.alloc_large(), Some(2 * MIB));
    }

    #[test]
    fn search_wraps_around_after_the_hint() {
        let mut bits = [0u64; 4];
        let mut fa = allocator(&mut bits, 200);
        fa.add_free_region(0, 200 * FRAME_SIZE);
        let all: Vec<_> = std::iter::from_fn(|| fa.alloc()).collect();
        assert_eq!(all.len(), 200);
        assert_eq!(
            all[199],
            199 * FRAME_SIZE,
            "frames are handed out in address order"
        );
        // Free one frame far below the hint (which now points past the end).
        fa.free(3 * FRAME_SIZE);
        assert_eq!(fa.alloc(), Some(3 * FRAME_SIZE));
        fa.free(130 * FRAME_SIZE);
        fa.free(131 * FRAME_SIZE);
        assert_eq!(fa.alloc_contiguous(2), Some(130 * FRAME_SIZE));
    }

    #[test]
    fn freed_frames_are_reused_before_untouched_ones() {
        let mut bits = [0u64; 8];
        let mut fa = allocator(&mut bits, 512);
        fa.add_free_region(0, 512 * FRAME_SIZE);
        for _ in 0..100 {
            fa.alloc().unwrap();
        }
        fa.free(40 * FRAME_SIZE);
        fa.free(7 * FRAME_SIZE);
        assert_eq!(
            fa.alloc(),
            Some(7 * FRAME_SIZE),
            "the lowest freed frame first"
        );
        assert_eq!(fa.alloc(), Some(40 * FRAME_SIZE));
        assert_eq!(
            fa.alloc(),
            Some(100 * FRAME_SIZE),
            "then frames never handed out"
        );
        fa.free_contiguous(10 * FRAME_SIZE, 3);
        assert_eq!(fa.alloc_contiguous(3), Some(10 * FRAME_SIZE));
    }

    #[test]
    fn contiguous_requests_at_the_edge_of_free_memory() {
        let mut bits = [0u64; 1];
        let mut fa = allocator(&mut bits, 64);
        fa.add_free_region(10 * FRAME_SIZE, 14 * FRAME_SIZE);
        assert_eq!(fa.alloc_contiguous(5), None, "more than is free");
        assert_eq!(fa.alloc_contiguous(0), None);
        assert_eq!(
            fa.alloc_contiguous(4),
            Some(10 * FRAME_SIZE),
            "exactly what is free"
        );
        assert_eq!(fa.free_frames(), 0);
    }

    #[test]
    fn usable_and_free_counts_follow_regions_and_reservations() {
        let mut bits = [0u64; 2];
        let mut fa = allocator(&mut bits, 128);
        fa.add_free_region(0, 100 * FRAME_SIZE);
        fa.add_free_region(50 * FRAME_SIZE, 120 * FRAME_SIZE); // overlaps
        assert_eq!((fa.usable_frames(), fa.free_frames()), (120, 120));
        fa.reserve(60 * FRAME_SIZE, 70 * FRAME_SIZE);
        assert_eq!((fa.usable_frames(), fa.free_frames()), (110, 110));
        // Free runs are now frames 0..60 and 70..120; 64 fits in neither.
        assert_eq!(fa.alloc_contiguous(64), None);
        assert_eq!(fa.alloc_contiguous(50), Some(0));
        assert_eq!((fa.usable_frames(), fa.free_frames()), (110, 60));
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
    #[should_panic(expected = "double free of frame 0x64000")]
    fn double_free_inside_a_run_names_its_lowest_free_frame() {
        let mut bits = [0u64; 4];
        let mut fa = allocator(&mut bits, 256);
        fa.add_free_region(0, 256 * FRAME_SIZE);
        // A run across three bitmap words, frames 60 to 139.
        let run = fa.alloc_contiguous(140).unwrap();
        fa.free(100 * FRAME_SIZE);
        fa.free(130 * FRAME_SIZE);
        fa.free_contiguous(run + 60 * FRAME_SIZE, 80);
    }

    #[test]
    fn runs_across_words_free_every_frame_once() {
        let mut bits = [0u64; 4];
        let mut fa = allocator(&mut bits, 256);
        fa.add_free_region(0, 256 * FRAME_SIZE);
        let run = fa.alloc_contiguous(200).unwrap();
        fa.free_contiguous(run + 3 * FRAME_SIZE, 190);
        assert_eq!(fa.free_frames(), 56 + 190);
        assert!(fa.is_allocated(run + 2 * FRAME_SIZE));
        assert!(!fa.is_allocated(run + 3 * FRAME_SIZE));
        assert!(!fa.is_allocated(run + 192 * FRAME_SIZE));
        assert!(fa.is_allocated(run + 193 * FRAME_SIZE));
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
        High,
        Free(usize),
    }

    proptest! {
        #[test]
        fn random_sequences_keep_invariants(
            ops in proptest::collection::vec(
                prop_oneof![
                    (1usize..8).prop_map(Op::Alloc),
                    Just(Op::High),
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
                    Op::High => match fa.alloc_high() {
                        Some(addr) => {
                            prop_assert!(!reserved.contains(&(addr / FRAME_SIZE)));
                            prop_assert!(owned.insert(addr), "duplicate frame");
                            live.push((addr, 1));
                        }
                        None => prop_assert_eq!(fa.free_frames(), 0),
                    },
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
