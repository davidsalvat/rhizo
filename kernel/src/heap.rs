//! The kernel's global allocator, backed by physical frames.

use crate::memory::{DirectMap, with_frames};
use crate::sync::Global;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::NonNull;
use rhizo_mm::DIRECT_MAP_BASE;
use rhizo_mm::heap::{Heap, PageSource};

/// Pages for the heap: contiguous frames, used through the direct map.
#[derive(Debug)]
struct FramePages;

impl PageSource for FramePages {
    fn alloc_pages(&mut self, count: usize) -> Option<NonNull<u8>> {
        let phys = with_frames(|f| f.alloc_contiguous(count)).flatten()?;
        NonNull::new(DirectMap::virt(phys) as *mut u8)
    }

    unsafe fn free_pages(&mut self, ptr: NonNull<u8>, count: usize) {
        let phys = ptr.as_ptr() as u64 - DIRECT_MAP_BASE;
        with_frames(|f| f.free_contiguous(phys, count));
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
