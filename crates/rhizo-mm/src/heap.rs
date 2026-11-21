//! Kernel heap: size classes for small blocks, whole pages for large ones
//! (design decision D6).
//!
//! Small requests (up to [`MAX_CLASS_SIZE`] bytes after rounding) are served
//! from per-class free lists. Each class carves 4 KiB pages into equal blocks.
//! The power-of-two classes (16 to 2048 bytes) serve every alignment up to
//! their size: a block of size `2^k` inside a page-aligned page is aligned to
//! `2^k`. Between them, classes of 48 to 1360 bytes, each a whole fraction of
//! a page, serve requests aligned to 16 bytes or less, so a request wastes
//! less than its size (design D1 of `heap-caches-cell-index`).
//! Larger requests take contiguous pages from the [`PageSource`] and return
//! them on free. Every operation is O(1) except page refills and trims.
//!
//! Free blocks are given back by page (design D1 of `add-heap-trim`): a
//! trim sorts each free list by address and returns to the page source
//! every page whose blocks are all free. The owner asks for one at quiet
//! moments ([`Heap::trim_if_due`]); allocating and freeing pay nothing.

use core::alloc::Layout;
use core::ptr::{self, NonNull};

/// Page size used by the heap.
pub const PAGE_SIZE: usize = 4096;
/// Smallest block size.
pub const MIN_CLASS_SIZE: usize = 16;
/// Largest block size served from size classes.
pub const MAX_CLASS_SIZE: usize = 2048;

/// Block size of each class: the powers of two first (their index is their
/// order above 16 bytes), then the classes between them, each the largest
/// multiple of 16 bytes of which a page holds 85, 42, 21, 12, 9, 6, 5 or 3.
const SIZES: [usize; CLASSES] = [
    16, 32, 64, 128, 256, 512, 1024, 2048, 48, 96, 192, 336, 448, 672, 816, 1360,
];
const CLASSES: usize = 16;

/// The smallest class holding `16 * i` bytes, for requests aligned to 16
/// bytes or less.
const CLASS_OF: [u8; MAX_CLASS_SIZE / MIN_CLASS_SIZE + 1] = {
    let mut table = [0u8; MAX_CLASS_SIZE / MIN_CLASS_SIZE + 1];
    let mut i = 0;
    while i < table.len() {
        let size = if i == 0 {
            MIN_CLASS_SIZE
        } else {
            i * MIN_CLASS_SIZE
        };
        let (mut best, mut c) = (0, 0);
        while c < CLASSES {
            if SIZES[c] >= size && (SIZES[best] < size || SIZES[c] < SIZES[best]) {
                best = c;
            }
            c += 1;
        }
        table[i] = best as u8;
        i += 1;
    }
    table
};

/// Bytes of class pages below which [`Heap::trim_if_due`] never trims.
pub const TRIM_AT: usize = 1 << 20;

/// Supplier of page-aligned, writable memory.
pub trait PageSource {
    /// Returns `count` contiguous, 4 KiB-aligned, writable pages.
    fn alloc_pages(&mut self, count: usize) -> Option<NonNull<u8>>;

    /// Gives back pages obtained from [`alloc_pages`](Self::alloc_pages).
    ///
    /// # Safety
    /// `ptr` and `count` must come from one earlier `alloc_pages` call, and
    /// the memory must not be used afterwards.
    unsafe fn free_pages(&mut self, ptr: NonNull<u8>, count: usize);
}

/// Allocation counters, useful for tests and reporting.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HeapStats {
    /// Pages currently obtained from the page source.
    pub pages: usize,
    /// Live allocations.
    pub live: usize,
}

/// How a layout is served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    Class(usize),
    Pages(usize),
}

fn plan(layout: Layout) -> Option<Plan> {
    if layout.align() > PAGE_SIZE {
        return None;
    }
    if layout.align() <= MIN_CLASS_SIZE && layout.size() <= MAX_CLASS_SIZE {
        // Every block is aligned to 16 bytes.
        let i = layout.size().div_ceil(MIN_CLASS_SIZE);
        return Some(Plan::Class(CLASS_OF[i] as usize));
    }
    // Only the power-of-two classes are aligned beyond 16 bytes.
    let size = layout.size().max(layout.align()).max(MIN_CLASS_SIZE);
    if size <= MAX_CLASS_SIZE {
        let rounded = size.next_power_of_two();
        Some(Plan::Class(
            (rounded.trailing_zeros() - MIN_CLASS_SIZE.trailing_zeros()) as usize,
        ))
    } else {
        Some(Plan::Pages(size.div_ceil(PAGE_SIZE)))
    }
}

/// Free block: the first word holds the next free block.
struct FreeBlock {
    next: Option<NonNull<FreeBlock>>,
}

/// The heap allocator.
#[derive(Debug)]
pub struct Heap<S: PageSource> {
    source: S,
    free: [Option<NonNull<u8>>; CLASSES],
    stats: HeapStats,
    /// Pages carved into blocks.
    class_pages: usize,
    /// Most live allocations [`trim_if_due`](Self::trim_if_due) saw since
    /// the last trim.
    peak_live: usize,
}

// SAFETY: the heap owns every block it hands out through raw pointers; moving
// it to another thread moves that ownership with it.
unsafe impl<S: PageSource + Send> Send for Heap<S> {}

impl<S: PageSource> Heap<S> {
    /// Creates an empty heap drawing pages from `source`.
    pub const fn new(source: S) -> Self {
        Self {
            source,
            free: [None; CLASSES],
            stats: HeapStats { pages: 0, live: 0 },
            class_pages: 0,
            peak_live: 0,
        }
    }

    /// Current counters.
    pub fn stats(&self) -> HeapStats {
        self.stats
    }

    /// Allocates memory for `layout`; returns null on failure.
    pub fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let ptr = match plan(layout) {
            None => ptr::null_mut(),
            Some(Plan::Class(class)) => self.alloc_block(class),
            Some(Plan::Pages(count)) => match self.source.alloc_pages(count) {
                Some(p) => {
                    self.stats.pages += count;
                    p.as_ptr()
                }
                None => ptr::null_mut(),
            },
        };
        if !ptr.is_null() {
            self.stats.live += 1;
        }
        ptr
    }

    /// Frees memory returned by [`alloc`](Self::alloc).
    ///
    /// # Safety
    /// `ptr` must have been returned by `alloc` on this heap with the same
    /// `layout`, and must not be used afterwards.
    pub unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        let Some(block) = NonNull::new(ptr) else {
            return;
        };
        self.stats.live -= 1;
        match plan(layout) {
            Some(Plan::Class(class)) => {
                let node = block.cast::<FreeBlock>();
                // SAFETY: the caller guarantees `block` is a live block of
                // this class, at least 16 bytes and 16-byte aligned, so it can
                // hold a `FreeBlock`.
                unsafe {
                    node.as_ptr().write(FreeBlock {
                        next: self.free[class].map(NonNull::cast),
                    })
                };
                self.free[class] = Some(block);
            }
            Some(Plan::Pages(count)) => {
                // SAFETY: the caller guarantees these pages came from
                // `alloc_pages(count)` for this layout.
                unsafe { self.source.free_pages(block, count) };
                self.stats.pages -= count;
            }
            None => unreachable!("alloc never succeeds for this layout"),
        }
    }

    fn alloc_block(&mut self, class: usize) -> *mut u8 {
        if self.free[class].is_none() && !self.refill(class) {
            return ptr::null_mut();
        }
        let block = self.free[class].expect("refilled class has a free block");
        // SAFETY: blocks on the free list are unused and hold a `FreeBlock`.
        let next = unsafe { block.cast::<FreeBlock>().as_ref().next };
        self.free[class] = next.map(NonNull::cast);
        block.as_ptr()
    }

    /// Trims ([`trim`](Self::trim)) if the class pages hold at least
    /// [`TRIM_AT`] bytes and the live allocations fell to half the most it
    /// saw since the last trim: what was freed since is worth sorting
    /// (design D2). Cheap otherwise: two comparisons.
    pub fn trim_if_due(&mut self) {
        self.peak_live = self.peak_live.max(self.stats.live);
        if self.class_pages * PAGE_SIZE >= TRIM_AT && 2 * self.stats.live <= self.peak_live {
            self.trim();
        }
    }

    /// Gives back every page whose blocks are all free.
    pub fn trim(&mut self) {
        for class in 0..CLASSES {
            self.trim_class(class);
        }
        self.peak_live = self.stats.live;
    }

    /// Sorts the free list of `class` by address, so the free blocks of one
    /// page are next to each other, and gives back the pages it holds whole.
    fn trim_class(&mut self, class: usize) {
        let size = SIZES[class];
        let page_of = |b: NonNull<u8>| b.as_ptr() as usize & !(PAGE_SIZE - 1);
        let mut rest = sort(self.free[class]);
        self.free[class] = None;
        let mut tail: Option<NonNull<u8>> = None;
        while let Some(first) = rest {
            // The run of free blocks in the page of `first`.
            let (mut last, mut blocks) = (first, 1);
            rest = next(first);
            while let Some(b) = rest.filter(|&b| page_of(b) == page_of(first)) {
                (last, blocks) = (b, blocks + 1);
                rest = next(b);
            }
            if blocks == PAGE_SIZE / size {
                let page = first.as_ptr().map_addr(|a| a & !(PAGE_SIZE - 1));
                let page = NonNull::new(page).expect("a page is never at 0");
                // SAFETY: every block of the page is free, so nothing uses
                // it; it came from `alloc_pages(1)` in `refill`.
                unsafe { self.source.free_pages(page, 1) };
                self.stats.pages -= 1;
                self.class_pages -= 1;
                continue;
            }
            match tail {
                // SAFETY: `t` is a free block of this class, kept on the list.
                Some(t) => unsafe { set_next(t, Some(first)) },
                None => self.free[class] = Some(first),
            }
            tail = Some(last);
        }
        if let Some(t) = tail {
            // SAFETY: as above.
            unsafe { set_next(t, None) };
        }
    }

    /// Carves one fresh page into blocks of `class` and pushes them.
    fn refill(&mut self, class: usize) -> bool {
        let Some(page) = self.source.alloc_pages(1) else {
            return false;
        };
        self.stats.pages += 1;
        self.class_pages += 1;
        let size = SIZES[class];
        // Push in reverse so blocks are handed out in address order.
        for i in (0..PAGE_SIZE / size).rev() {
            // SAFETY: `i * size` is inside the fresh page, and so is the
            // block: the page holds `PAGE_SIZE / size` whole blocks.
            let block = unsafe { page.add(i * size) };
            let node = block.cast::<FreeBlock>();
            // SAFETY: the block is unused, aligned to 16 bytes (`size` is a
            // multiple of 16, the page is page-aligned) and large enough for
            // a `FreeBlock`.
            unsafe {
                node.as_ptr().write(FreeBlock {
                    next: self.free[class].map(NonNull::cast),
                })
            };
            self.free[class] = Some(block);
        }
        true
    }
}

/// The block after free block `b` on its list.
fn next(b: NonNull<u8>) -> Option<NonNull<u8>> {
    // SAFETY: only free blocks are passed, and each holds a `FreeBlock`.
    unsafe { b.cast::<FreeBlock>().as_ref().next.map(NonNull::cast) }
}

/// Links free block `b` to `next`.
///
/// # Safety
/// `b` must be a free block of the heap (it holds a `FreeBlock`).
unsafe fn set_next(b: NonNull<u8>, next: Option<NonNull<u8>>) {
    // SAFETY: the caller guarantees `b` is free and holds a `FreeBlock`.
    unsafe {
        b.cast::<FreeBlock>().as_ptr().write(FreeBlock {
            next: next.map(NonNull::cast),
        })
    };
}

/// Sorts a free list by address, in place: a merge sort of the list's own
/// links (runs of 1, 2, 4... blocks merged pairwise), O(n log n) steps and
/// no memory.
fn sort(mut head: Option<NonNull<u8>>) -> Option<NonNull<u8>> {
    let mut run = 1;
    loop {
        let (mut p, mut merges) = (head, 0);
        let (mut out, mut tail): (Option<NonNull<u8>>, Option<NonNull<u8>>) = (None, None);
        while p.is_some() {
            merges += 1;
            // `p` takes up to `run` blocks; `q` starts after them.
            let mut q = p;
            let mut p_left = 0;
            while p_left < run && q.is_some() {
                p_left += 1;
                q = q.and_then(next);
            }
            let mut q_left = run;
            while p_left > 0 || (q_left > 0 && q.is_some()) {
                let take_p = match (p_left > 0, q.filter(|_| q_left > 0)) {
                    (true, Some(qb)) => p.is_some_and(|pb| pb <= qb),
                    (true, None) => true,
                    (false, _) => false,
                };
                let b = if take_p {
                    p_left -= 1;
                    let b = p.expect("counted in p_left");
                    p = next(b);
                    b
                } else {
                    q_left -= 1;
                    let b = q.expect("checked above");
                    q = next(b);
                    b
                };
                match tail {
                    // SAFETY: `t` is a free block on the list being built.
                    Some(t) => unsafe { set_next(t, Some(b)) },
                    None => out = Some(b),
                }
                tail = Some(b);
            }
            p = q;
        }
        if let Some(t) = tail {
            // SAFETY: as above.
            unsafe { set_next(t, None) };
        }
        head = out;
        if merges <= 1 {
            return head;
        }
        run *= 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::alloc::{alloc, dealloc};

    /// Page source backed by the host allocator, with an optional limit.
    /// The heap keeps size-class pages until a trim, so the double returns
    /// whatever is still lent out when it is dropped.
    #[derive(Default)]
    struct HostPages {
        outstanding: usize,
        limit: Option<usize>,
        lent: Vec<(NonNull<u8>, usize)>,
    }

    impl Drop for HostPages {
        fn drop(&mut self) {
            for (ptr, count) in self.lent.drain(..) {
                let layout = Layout::from_size_align(count * PAGE_SIZE, PAGE_SIZE).unwrap();
                // SAFETY: every entry came from `alloc` with this layout and was
                // not freed yet (freed entries are removed in `free_pages`).
                unsafe { dealloc(ptr.as_ptr(), layout) };
            }
        }
    }

    impl PageSource for HostPages {
        fn alloc_pages(&mut self, count: usize) -> Option<NonNull<u8>> {
            if self.limit.is_some_and(|l| self.outstanding + count > l) {
                return None;
            }
            self.outstanding += count;
            let layout = Layout::from_size_align(count * PAGE_SIZE, PAGE_SIZE).unwrap();
            // SAFETY: non-zero size.
            let ptr = NonNull::new(unsafe { alloc(layout) })?;
            self.lent.push((ptr, count));
            Some(ptr)
        }

        unsafe fn free_pages(&mut self, ptr: NonNull<u8>, count: usize) {
            self.outstanding -= count;
            self.lent.retain(|&(p, _)| p != ptr);
            let layout = Layout::from_size_align(count * PAGE_SIZE, PAGE_SIZE).unwrap();
            // SAFETY: same layout as the allocation.
            unsafe { dealloc(ptr.as_ptr(), layout) };
        }
    }

    fn layout(size: usize, align: usize) -> Layout {
        Layout::from_size_align(size, align).unwrap()
    }

    /// Frees a block that the test allocated from `heap` with layout `l`.
    fn free(heap: &mut Heap<HostPages>, p: *mut u8, l: Layout) {
        // SAFETY: tests only pass pointers they got from `heap.alloc(l)` and
        // never use them again.
        unsafe { heap.dealloc(p, l) }
    }

    /// Reads byte `k` of a live block of at least `k + 1` bytes.
    fn byte(p: *mut u8, k: usize) -> u8 {
        // SAFETY: tests only call this for in-bounds offsets of live blocks.
        unsafe { *p.add(k) }
    }

    /// Fills a live block of at least `len` bytes with `tag`.
    fn fill(p: *mut u8, tag: u8, len: usize) {
        // SAFETY: tests only call this with the size of a live block.
        unsafe { ptr::write_bytes(p, tag, len) }
    }

    #[test]
    fn plans() {
        assert_eq!(plan(layout(1, 1)), Some(Plan::Class(0)));
        assert_eq!(plan(layout(17, 1)), Some(Plan::Class(1)));
        assert_eq!(plan(layout(8, 64)), Some(Plan::Class(2)));
        assert_eq!(plan(layout(2048, 8)), Some(Plan::Class(7)));
        assert_eq!(plan(layout(2049, 8)), Some(Plan::Pages(1)));
        assert_eq!(plan(layout(1, 4096)), Some(Plan::Pages(1)));
        assert_eq!(plan(layout(8193, 8)), Some(Plan::Pages(3)));
        assert_eq!(plan(layout(8, 8192)), None);
    }

    #[test]
    fn classes_between_the_powers_of_two_hold_whole_blocks() {
        for (c, &size) in SIZES.iter().enumerate() {
            assert_eq!(size % MIN_CLASS_SIZE, 0, "class {c}");
            if c >= 8 {
                // The largest multiple of 16 for its number of blocks.
                let blocks = PAGE_SIZE / size;
                assert!(PAGE_SIZE / blocks - size < MIN_CLASS_SIZE, "class {c}");
            }
        }
        assert_eq!(plan(layout(33, 8)), Some(Plan::Class(8)), "48 bytes");
        assert_eq!(plan(layout(48, 16)), Some(Plan::Class(8)));
        assert_eq!(plan(layout(48, 32)), Some(Plan::Class(2)), "aligned: 64");
        assert_eq!(plan(layout(1350, 8)), Some(Plan::Class(15)), "1360 bytes");
        assert_eq!(plan(layout(1361, 8)), Some(Plan::Class(7)), "2048 bytes");
        assert_eq!(plan(layout(0, 1)), Some(Plan::Class(0)));
    }

    #[test]
    fn three_blocks_of_a_third_of_a_page_share_one() {
        let mut heap = Heap::new(HostPages::default());
        let l = layout(1350, 8);
        let blocks: Vec<_> = (0..3).map(|_| heap.alloc(l)).collect();
        assert_eq!(heap.stats().pages, 1, "a power-of-two class would take two");
        for (i, &p) in blocks.iter().enumerate() {
            fill(p, i as u8 + 1, l.size());
        }
        for (i, &p) in blocks.iter().enumerate() {
            assert!((0..l.size()).all(|k| byte(p, k) == i as u8 + 1));
            free(&mut heap, p, l);
        }
        heap.trim();
        assert_eq!(heap.stats(), HeapStats { pages: 0, live: 0 });
    }

    #[test]
    fn alignment_is_honored() {
        let mut heap = Heap::new(HostPages::default());
        for align in [1, 8, 64, 4096] {
            let l = layout(24, align);
            let p = heap.alloc(l);
            assert!(!p.is_null());
            assert_eq!(p as usize % align, 0, "align {align}");
            free(&mut heap, p, l);
        }
        assert!(heap.alloc(layout(8, 8192)).is_null());
    }

    #[test]
    fn freed_block_is_reused_without_new_pages() {
        let mut heap = Heap::new(HostPages::default());
        let l = layout(100, 8);
        let a = heap.alloc(l);
        let pages = heap.stats().pages;
        free(&mut heap, a, l);
        let b = heap.alloc(l);
        assert_eq!(a, b);
        assert_eq!(heap.stats().pages, pages);
    }

    #[test]
    fn large_allocations_return_their_pages() {
        let mut heap = Heap::new(HostPages::default());
        let l = layout(3 * PAGE_SIZE + 1, 16);
        let p = heap.alloc(l);
        assert_eq!(heap.stats().pages, 4);
        free(&mut heap, p, l);
        assert_eq!(heap.stats(), HeapStats { pages: 0, live: 0 });
    }

    #[test]
    fn exhaustion_returns_null() {
        let mut heap = Heap::new(HostPages {
            outstanding: 0,
            limit: Some(2),
            lent: Vec::new(),
        });
        assert!(heap.alloc(layout(3 * PAGE_SIZE, 8)).is_null());
        let l = layout(2048, 8);
        let blocks: Vec<_> = (0..4).map(|_| heap.alloc(l)).collect();
        assert!(blocks.iter().all(|p| !p.is_null()));
        assert!(heap.alloc(layout(PAGE_SIZE, 8)).is_null());
    }

    #[test]
    fn whole_free_pages_go_back_when_due() {
        let mut heap = Heap::new(HostPages::default());
        let l = layout(MAX_CLASS_SIZE, 8);
        let n = 4 * TRIM_AT / MAX_CLASS_SIZE;
        let blocks: Vec<_> = (0..n).map(|_| heap.alloc(l)).collect();
        heap.trim_if_due(); // sees the peak: nothing freed, no trim
        let peak = heap.stats().pages;
        assert_eq!(peak, 4 * TRIM_AT / PAGE_SIZE);
        // Fewer than half freed: not due.
        for &p in &blocks[..n / 2 - 1] {
            free(&mut heap, p, l);
        }
        heap.trim_if_due();
        assert_eq!(heap.stats().pages, peak, "not due yet");
        for &p in &blocks[n / 2 - 1..] {
            free(&mut heap, p, l);
        }
        heap.trim_if_due();
        assert_eq!(heap.stats(), HeapStats { pages: 0, live: 0 });
        assert_eq!(heap.source.outstanding, 0, "every page back to its source");
    }

    #[test]
    fn a_small_heap_is_never_trimmed_when_due() {
        let mut heap = Heap::new(HostPages::default());
        let l = layout(64, 8);
        let p = heap.alloc(l);
        heap.trim_if_due();
        free(&mut heap, p, l);
        heap.trim_if_due();
        assert_eq!(heap.stats().pages, 1, "kept below TRIM_AT");
    }

    #[test]
    fn a_page_with_a_live_block_stays() {
        let mut heap = Heap::new(HostPages::default());
        let l = layout(1024, 8);
        let blocks: Vec<_> = (0..12).map(|_| heap.alloc(l)).collect();
        assert_eq!(heap.stats().pages, 3);
        // One live block in each of the first two pages, none in the third.
        let keep = [blocks[1], blocks[6]];
        fill(keep[0], 0xa5, 1024);
        fill(keep[1], 0x5a, 1024);
        for &p in blocks.iter().filter(|p| !keep.contains(p)) {
            free(&mut heap, p, l);
        }
        heap.trim();
        assert_eq!(heap.stats().pages, 2, "the empty page went back");
        assert!((0..1024).all(|k| byte(keep[0], k) == 0xa5 && byte(keep[1], k) == 0x5a));
        // The six free blocks of the two pages are reused, in address order.
        let again: Vec<_> = (0..6).map(|_| heap.alloc(l)).collect();
        let mut sorted = again.clone();
        sorted.sort();
        assert_eq!(again, sorted);
        assert_eq!(heap.stats().pages, 2, "no new page");
        for p in again.into_iter().chain(keep) {
            free(&mut heap, p, l);
        }
    }

    proptest! {
        #[test]
        fn a_request_gets_the_smallest_class_that_fits(size in 0usize..3000, align_pow in 0u32..13) {
            let l = layout(size, 1 << align_pow);
            let fits = |c: usize| {
                SIZES[c] >= size.max(l.align())
                    && (l.align() <= MIN_CLASS_SIZE || SIZES[c].is_power_of_two())
            };
            let best = (0..CLASSES).filter(|&c| fits(c)).min_by_key(|&c| SIZES[c]);
            match (plan(l), best) {
                (Some(Plan::Class(c)), Some(b)) => prop_assert_eq!(SIZES[c], SIZES[b]),
                (Some(Plan::Pages(n)), None) => {
                    prop_assert_eq!(n, size.max(l.align()).div_ceil(PAGE_SIZE));
                }
                (got, best) => prop_assert!(false, "{:?} for {:?}, best {:?}", got, l, best),
            }
        }

        #[test]
        fn sort_orders_a_free_list(order in Just((0..300usize).collect::<Vec<_>>()).prop_shuffle()) {
            let mut buf = vec![0u128; 300];
            let base = buf.as_mut_ptr().cast::<u8>();
            // SAFETY: each block is a 16-byte slot of `buf`, which outlives
            // the list.
            let block = |i: usize| NonNull::new(unsafe { base.add(16 * i) }).unwrap();
            let mut head = None;
            for &i in &order {
                // SAFETY: as above; the slot holds a `FreeBlock` from now on.
                unsafe { set_next(block(i), head) };
                head = Some(block(i));
            }
            let mut got = Vec::new();
            let mut b = sort(head);
            while let Some(x) = b {
                got.push(x);
                b = next(x);
            }
            prop_assert_eq!(got, (0..300).map(block).collect::<Vec<_>>());
        }

        #[test]
        fn allocations_never_overlap(
            ops in proptest::collection::vec((1usize..6000, 0u32..13, any::<bool>()), 1..300)
        ) {
            let mut heap = Heap::new(HostPages::default());
            let mut live: Vec<(*mut u8, Layout, u8)> = Vec::new();
            for (i, (size, align_pow, free_one)) in ops.into_iter().enumerate() {
                if i % 16 == 15 {
                    heap.trim();
                }
                if free_one && !live.is_empty() {
                    let (p, l, tag) = live.swap_remove(i % live.len());
                    // The block must still hold its pattern: nobody overlapped it.
                    for k in 0..l.size() {
                        prop_assert_eq!(byte(p, k), tag);
                    }
                    free(&mut heap, p, l);
                    continue;
                }
                let l = layout(size, 1 << align_pow);
                let p = heap.alloc(l);
                prop_assert!(!p.is_null());
                prop_assert_eq!(p as usize % l.align(), 0);
                let tag = (i % 251) as u8;
                fill(p, tag, l.size());
                live.push((p, l, tag));
            }
            for (p, l, tag) in live {
                for k in 0..l.size() {
                    prop_assert_eq!(byte(p, k), tag);
                }
                free(&mut heap, p, l);
            }
            prop_assert_eq!(heap.stats().live, 0);
            heap.trim();
            prop_assert_eq!(heap.stats().pages, 0);
        }
    }
}
