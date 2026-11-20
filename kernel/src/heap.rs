//! The kernel's global allocator, backed by physical frames.

use crate::memory::{DirectMap, with_frames};
use crate::sync::Global;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::NonNull;
use rhizo_mm::DIRECT_MAP_BASE;
use rhizo_mm::heap::{Heap, PageSource};

/// Pages for the heap: contiguous frames, used through the direct map.
/// Single pages (most of them carved into small blocks, and kept) come from
/// the top of memory, so they do not split the free runs that large
/// allocations such as snapshots need.
#[derive(Debug)]
struct FramePages;

impl PageSource for FramePages {
    fn alloc_pages(&mut self, count: usize) -> Option<NonNull<u8>> {
        let phys = with_frames(|f| match count {
            1 => f.alloc_high(),
            _ => f.alloc_contiguous(count),
        })
        .flatten();
        #[cfg(any(feature = "ktest", feature = "bench-cell"))]
        let Some(phys) = phys else {
            return window::alloc(count);
        };
        #[cfg(not(any(feature = "ktest", feature = "bench-cell")))]
        let phys = phys?;
        NonNull::new(DirectMap::virt(phys) as *mut u8)
    }

    unsafe fn free_pages(&mut self, ptr: NonNull<u8>, count: usize) {
        #[cfg(any(feature = "ktest", feature = "bench-cell"))]
        if window::contains(ptr) {
            return window::free(ptr, count);
        }
        let phys = ptr.as_ptr() as u64 - DIRECT_MAP_BASE;
        with_frames(|f| f.free_contiguous(phys, count));
    }
}

/// Large heap allocations when no run of contiguous frames is left (design
/// D7 of `add-smp-scheduling`): single frames mapped one after the other in
/// a window of the kernel half. Addresses are handed out once and never
/// reused, so no CPU can reach new memory through a stale translation of a
/// freed allocation.
#[cfg(any(feature = "ktest", feature = "bench-cell"))]
pub mod window {
    use crate::memory::{self, with_frames};
    use core::ptr::NonNull;
    use core::sync::atomic::{AtomicU64, Ordering};
    use rhizo_mm::FRAME_SIZE;
    use rhizo_mm::paging::PageFlags;

    /// The window: one top-level entry (512 GiB) of the kernel half.
    const BASE: u64 = 0xffff_9000_0000_0000;
    const END: u64 = BASE + (512 << 30);
    /// Next address handed out.
    static NEXT: AtomicU64 = AtomicU64::new(BASE);

    /// Creates the window's tables, so cell roots that copy the kernel's
    /// top-level entries (x86_64) see what is mapped there later. Call
    /// before the first cell exists.
    pub fn init() {
        let phys = with_frames(|f| f.alloc()).flatten().expect("a frame");
        memory::map_page(BASE, phys, PageFlags::WRITABLE | PageFlags::NO_EXECUTE)
            .expect("map the heap window");
        let _ = memory::unmap_page(BASE);
        with_frames(|f| f.free(phys));
    }

    pub(super) fn contains(ptr: NonNull<u8>) -> bool {
        (BASE..END).contains(&(ptr.as_ptr() as u64))
    }

    pub(super) fn alloc(count: usize) -> Option<NonNull<u8>> {
        let len = count as u64 * FRAME_SIZE;
        let start = NEXT.fetch_add(len, Ordering::Relaxed);
        if start + len > END {
            return None;
        }
        for (i, virt) in (start..start + len)
            .step_by(FRAME_SIZE as usize)
            .enumerate()
        {
            let mapped = with_frames(|f| f.alloc())
                .flatten()
                .filter(|&phys| {
                    let flags = PageFlags::WRITABLE | PageFlags::NO_EXECUTE;
                    let ok = memory::map_page(virt, phys, flags).is_ok();
                    if !ok {
                        with_frames(|f| f.free(phys));
                    }
                    ok
                })
                .is_some();
            if !mapped {
                unmap(start, i);
                return None;
            }
        }
        NonNull::new(start as *mut u8)
    }

    pub(super) fn free(ptr: NonNull<u8>, count: usize) {
        unmap(ptr.as_ptr() as u64, count);
    }

    /// Unmaps `count` pages from `start` and frees their frames.
    fn unmap(start: u64, count: usize) {
        for i in 0..count as u64 {
            if let Ok(phys) = memory::unmap_page(start + i * FRAME_SIZE) {
                with_frames(|f| f.free(phys));
            }
        }
    }
}

struct KernelAllocator {
    heap: Global<Heap<FramePages>>,
}

// SAFETY: `Heap` upholds the `GlobalAlloc` contract (aligned, non-overlapping
// blocks, null on failure), and `Global` serializes access.
unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.heap.with(|heap| heap.alloc(layout))
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwarded from the caller's `GlobalAlloc::dealloc` contract.
        self.heap.with(|heap| unsafe { heap.dealloc(ptr, layout) })
    }
}

#[global_allocator]
static ALLOCATOR: KernelAllocator = KernelAllocator {
    heap: Global::new(Heap::new(FramePages)),
};

/// Pages the kernel heap holds and its live allocations.
#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests")
)]
pub fn stats() -> rhizo_mm::heap::HeapStats {
    ALLOCATOR.heap.with(|heap| heap.stats())
}
