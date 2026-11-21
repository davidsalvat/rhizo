//! Kernel heap: power-of-two size classes for small blocks, whole pages for
//! large ones (design decision D6).
//!
//! Small requests (up to [`MAX_CLASS_SIZE`] bytes after rounding) are served
//! from per-class free lists. Each class carves 4 KiB pages into equal blocks;
//! a block of size `2^k` inside a page-aligned page is aligned to `2^k`.
//! Larger requests take contiguous pages from the [`PageSource`] and return
//! them on free. Every operation is O(1) except page refills and trims.
//!
//! Free blocks are given back by page (design D1 of `add-heap-trim`): once
//! the free blocks of the classes reach [`TRIM_AT`] bytes, and twice what
//! was left after the last trim, a trim sorts each free list by address and
//! returns to the page source every page whose blocks are all free.

use core::alloc::Layout;
use core::ptr::{self, NonNull};

/// Page size used by the heap.
pub const PAGE_SIZE: usize = 4096;
/// Smallest block size.
pub const MIN_CLASS_SIZE: usize = 16;
/// Largest block size served from size classes.
pub const MAX_CLASS_SIZE: usize = 2048;

const CLASSES: usize = 8; // 16, 32, 64, 128, 256, 512, 1024, 2048

/// Free bytes in the classes below which no trim runs.
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
    /// Bytes of the blocks on the free lists.
    free_bytes: usize,
    /// The next trim runs when `free_bytes` reaches this.
    trim_at: usize,
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
            free_bytes: 0,
            trim_at: TRIM_AT,
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
                self.free_bytes += MIN_CLASS_SIZE << class;
                if self.free_bytes >= self.trim_at {
                    self.trim();
                }
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
        self.free_bytes -= MIN_CLASS_SIZE << class;
        block.as_ptr()
    }

    /// Gives back every page whose blocks are all free, and sets the next
    /// trim at twice what is left (at least [`TRIM_AT`]): each trim follows
    /// at least as many bytes freed as it sorts, half of them (design D1).
    pub fn trim(&mut self) {
        for class in 0..CLASSES {
            self.trim_class(class);
        }
        self.trim_at = (2 * self.free_bytes).max(TRIM_AT);
    }

    /// Sorts the free list of `class` by address, so the free blocks of one
    /// page are next to each other, and gives back the pages it holds whole.
    fn trim_class(&mut self, class: usize) {
        let size = MIN_CLASS_SIZE << class;
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
                self.free_bytes -= PAGE_SIZE;
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
        self.free_bytes += PAGE_SIZE;
        let size = MIN_CLASS_SIZE << class;
        // Push in reverse so blocks are handed out in address order.
        for i in (0..PAGE_SIZE / size).rev() {
            // SAFETY: `i * size` is inside the fresh page.
            let block = unsafe { page.add(i * size) };
            let node = block.cast::<FreeBlock>();
            // SAFETY: the block is unused, aligned to `size` (>= 16) and large
            // enough for a `FreeBlock`.
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
    /// The heap keeps size-class pages forever (by design), so the double
    /// returns whatever is still lent out when it is dropped.
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
    fn whole_free_pages_go_back() {
        let mut heap = Heap::new(HostPages::default());
        let l = layout(MAX_CLASS_SIZE, 8);
        let blocks: Vec<_> = (0..4 * TRIM_AT / MAX_CLASS_SIZE)
            .map(|_| heap.alloc(l))
            .collect();
        let peak = heap.stats().pages;
        assert_eq!(peak, 4 * TRIM_AT / PAGE_SIZE);
        for &p in &blocks {
            free(&mut heap, p, l);
        }
        let kept = heap.stats().pages;
        assert!(
            kept * PAGE_SIZE <= TRIM_AT,
            "{kept} pages kept without an explicit trim"
        );
        heap.trim();
        assert_eq!(heap.stats(), HeapStats { pages: 0, live: 0 });
        assert_eq!(heap.source.outstanding, 0, "every page back to its source");
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
