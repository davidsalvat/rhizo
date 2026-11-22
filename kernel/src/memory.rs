//! Kernel memory setup: memory map → frames → stage-2 page tables → heap.

use crate::arch::boot::layout;
use crate::arch::cpu::{self, CpuTlb};
use crate::arch::mem;
use crate::arch::platform::{self, Format};
use crate::sync::Global;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use rhizo_hal::PhysicalMemory;
use rhizo_mm::frames::FrameAllocator;
use rhizo_mm::paging::{MapError, PageFlags, PageSize, PageTable, Translation, UnmapError};
use rhizo_mm::{DIRECT_MAP_BASE, FRAME_SIZE, MAX_PHYS, align_down, align_up};

const BITMAP_WORDS: usize = (MAX_PHYS / FRAME_SIZE / 64) as usize;

/// Physical memory accessed through the direct map.
#[derive(Debug, Clone, Copy)]
pub struct DirectMap;

impl DirectMap {
    /// Virtual address of physical address `phys`.
    pub const fn virt(phys: u64) -> u64 {
        DIRECT_MAP_BASE + phys
    }
}

impl PhysicalMemory for DirectMap {
    fn read_u64(&self, phys: u64) -> u64 {
        // SAFETY: all RAM below 4 GiB is mapped at DIRECT_MAP_BASE (stage 1 and
        // stage 2), callers only pass RAM addresses, and they are 8-aligned.
        unsafe { (Self::virt(phys) as *const u64).read() }
    }

    fn write_u64(&mut self, phys: u64, value: u64) {
        // SAFETY: as for `read_u64`; page-table and frame memory written here
        // is owned by the memory subsystem.
        unsafe { (Self::virt(phys) as *mut u64).write(value) }
    }

    fn zero_frame(&mut self, frame: u64) {
        let _copy = copying();
        // SAFETY: the frame is 4 KiB-aligned RAM (normal memory), mapped
        // through the direct map, and owned by the caller.
        unsafe { mem::zero_page(Self::virt(frame) as *mut u8) }
    }

    fn read_bytes(&self, phys: u64, out: &mut [u8]) {
        let _copy = copying();
        let src = Self::virt(phys) as *const u8;
        // SAFETY: as for `read_u64`; the range is RAM owned by the caller and
        // does not overlap `out` (kernel heap or stack).
        unsafe { core::ptr::copy_nonoverlapping(src, out.as_mut_ptr(), out.len()) }
    }

    fn write_bytes(&mut self, phys: u64, data: &[u8]) {
        let _copy = copying();
        let dst = Self::virt(phys) as *mut u8;
        // SAFETY: as for `write_u64`; the range is RAM owned by the caller and
        // does not overlap `data`.
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len()) }
    }

    fn read_pages(&self, phys: u64, out: &mut [u8]) {
        let _copy = copying();
        let src = Self::virt(phys) as *const u8;
        // SAFETY: as for `read_bytes`.
        unsafe { copy(out.as_mut_ptr(), src, out.len()) }
    }

    fn write_pages(&mut self, phys: u64, data: &[u8]) {
        let _copy = copying();
        let dst = Self::virt(phys) as *mut u8;
        // SAFETY: as for `write_bytes`.
        unsafe { copy(dst, data.as_ptr(), data.len()) }
    }

    fn copy(&mut self, from: u64, to: u64, len: u64) {
        let _copy = copying();
        // SAFETY: both ranges are RAM mapped through the direct map, owned by
        // the caller, and do not overlap.
        unsafe {
            copy(
                Self::virt(to) as *mut u8,
                Self::virt(from) as *const u8,
                len as usize,
            )
        }
    }
}

/// Copies `len` bytes from `src` to `dst`: whole pages with the
/// architecture's page copy, anything else with `memcpy` (change
/// `fast-mem-primitives`).
///
/// # Safety
/// As for `core::ptr::copy_nonoverlapping`, on normal memory.
#[inline]
unsafe fn copy(dst: *mut u8, src: *const u8, len: usize) {
    // Callers pass whole pages; anything else is copied all the same.
    if len < PAGE || !len.is_multiple_of(PAGE) {
        // SAFETY: guaranteed by the caller.
        return unsafe { core::ptr::copy_nonoverlapping(src, dst, len) };
    }
    // SAFETY: guaranteed by the caller; the page copy takes any alignment.
    unsafe { mem::copy_pages(dst, src, len / PAGE) }
}

/// The unit of the architecture's zero and copy primitives.
const PAGE: usize = FRAME_SIZE as usize;

/// Accounts the time of a copy as such (spec `profiling`) in profile
/// builds, until the returned value is dropped.
#[inline]
fn copying() -> impl Sized {
    #[cfg(feature = "profile")]
    return crate::cost::scope(rhizo_prof::Part::Copy);
    #[cfg(not(feature = "profile"))]
    ()
}

static BITMAP: Global<[u64; BITMAP_WORDS]> = Global::new([0; BITMAP_WORDS]);
static BITMAP_TAKEN: AtomicBool = AtomicBool::new(false);
static FRAMES: Global<Option<FrameAllocator<'static>>> = Global::new(None);
static KERNEL_ROOT: AtomicU64 = AtomicU64::new(0);

/// Memory figures printed at boot.
#[derive(Debug, Clone, Copy)]
pub struct Summary {
    /// Bytes of RAM made available to the allocator.
    pub usable: u64,
    /// Bytes still free after boot setup.
    pub free: u64,
    /// Frames used by the kernel's page tables.
    pub page_table_frames: u64,
}

fn take_bitmap() -> &'static mut [u64] {
    assert!(
        !BITMAP_TAKEN.swap(true, Ordering::AcqRel),
        "memory already initialized"
    );
    let ptr = BITMAP.with(|b| b.as_mut_ptr());
    // SAFETY: handed out exactly once (checked above) and never accessed
    // through `BITMAP` again.
    unsafe { core::slice::from_raw_parts_mut(ptr, BITMAP_WORDS) }
}

/// Most RAM regions the kernel tracks for its direct map.
const MAX_RAM_REGIONS: usize = 16;

/// Sets up physical memory and the kernel's final address space.
pub fn init() -> Summary {
    let mut mem = DirectMap;

    // The bitmap is a zeroed static (all frames in use), so no clearing pass.
    let mut frames = FrameAllocator::from_zeroed(take_bitmap(), (MAX_PHYS / FRAME_SIZE) as usize);
    let mut ram = [(0u64, 0u64); MAX_RAM_REGIONS];
    let mut ram_count = 0;
    platform::for_each_ram(|start, end| {
        let end = end.min(MAX_PHYS);
        if start < end && ram_count < MAX_RAM_REGIONS {
            frames.add_free_region(start, end);
            ram[ram_count] = (start, end);
            ram_count += 1;
        }
    });
    let (kernel_start, kernel_end) = layout::kernel_phys();
    frames.reserve(kernel_start, kernel_end);
    platform::for_each_reserved(|start, end| frames.reserve(start, end));
    let usable = frames.usable_frames() as u64 * FRAME_SIZE;

    let before = frames.free_frames();
    let root = build_kernel_tables(&mut mem, &mut frames, &ram[..ram_count])
        .unwrap_or_else(|e| panic!("cannot build kernel page tables: {e:?}"));
    let mut alloc = || frames.alloc();
    // SAFETY: the new tables map the kernel image (code, data, stack) at the
    // same virtual addresses and the direct map at the same offset.
    unsafe { cpu::activate(root.root(), &mut alloc) };
    let page_table_frames = (before - frames.free_frames()) as u64;
    KERNEL_ROOT.store(root.root(), Ordering::Release);

    FRAMES.with(|slot| *slot = Some(frames));
    Summary {
        usable,
        free: free_bytes(),
        page_table_frames,
    }
}

fn build_kernel_tables(
    mem: &mut DirectMap,
    frames: &mut FrameAllocator<'static>,
    ram: &[(u64, u64)],
) -> Result<PageTable<Format>, MapError> {
    let mut alloc = || frames.alloc();
    let pt = PageTable::new(mem, &mut alloc)?;

    // Direct map of RAM: the largest pages the CPU supports (fewest table
    // frames and TLB entries), read/write, never executable. Regions are
    // rounded out to whole pages; a page already mapped by a neighbouring
    // region is skipped.
    let rw_nx = PageFlags::WRITABLE | PageFlags::NO_EXECUTE;
    let page = if cpu::supports_1gib_pages() {
        PageSize::Huge
    } else {
        PageSize::Large
    };
    for &(start, end) in ram {
        let first = align_down(start, page.bytes());
        for phys in (first..align_up(end, page.bytes())).step_by(page.bytes() as usize) {
            match pt.map(mem, &mut alloc, DirectMap::virt(phys), phys, page, rw_nx) {
                Ok(()) | Err(MapError::AlreadyMapped(_)) => {}
                Err(e) => return Err(e),
            }
        }
    }

    // Kernel image with W^X: code RX, read-only data R, data RW (no guard page).
    let guard = layout::stack_guard();
    let sections = [
        (layout::text(), PageFlags::empty()),
        (layout::rodata(), PageFlags::NO_EXECUTE),
        (layout::data(), rw_nx),
    ];
    for ((start, end), flags) in sections {
        for virt in (start..end).step_by(FRAME_SIZE as usize) {
            if virt != guard {
                pt.map(
                    mem,
                    &mut alloc,
                    virt,
                    layout::virt_to_phys(virt),
                    PageSize::Small,
                    flags,
                )?;
            }
        }
    }

    // Devices the kernel keeps using (for example a memory-mapped UART).
    platform::map_devices(&pt, &mut alloc)?;
    Ok(pt)
}

/// The kernel's page tables.
pub fn kernel_tables() -> PageTable<Format> {
    let root = KERNEL_ROOT.load(Ordering::Acquire);
    assert!(root != 0, "memory not initialized");
    PageTable::from_root(root)
}

/// Runs `f` with the frame allocator, or returns `None` before memory setup.
pub fn with_frames<R>(f: impl FnOnce(&mut FrameAllocator<'static>) -> R) -> Option<R> {
    FRAMES.with(|slot| slot.as_mut().map(f))
}

/// How often CPUs waited for the frame allocator (spec `profiling`).
#[cfg(all(feature = "bench-cell", feature = "profile"))]
pub fn contention() -> rhizo_core::sync::Contention {
    FRAMES.contention()
}

/// Bytes currently free in the frame allocator.
pub fn free_bytes() -> u64 {
    with_frames(|f| f.free_frames() as u64 * FRAME_SIZE).unwrap_or(0)
}

/// Frames kept for the kernel heap (design D1 of `add-memory-accounting`):
/// 8 MiB, or an eighth of memory if that is less.
#[cfg(feature = "cells")]
fn reserve_frames(f: &FrameAllocator<'_>) -> usize {
    (f.usable_frames() / 8).min((8 << 20) / FRAME_SIZE as usize)
}

/// Frames set aside for the cells of reserved domains (design D4 of
/// `add-cell-hierarchy`): what their memory limits leave unused. Their
/// meters join it, and take what they count out of it first, so only the
/// kernel's caches and cells without a memory limit are kept off it.
#[cfg(feature = "cells")]
pub static RESERVE: rhizo_cell::quota::Reserve = rhizo_cell::quota::Reserve::new();

/// Frames cells and the kernel's caches may not take: the kernel's
/// reserve, and what is set aside for reserved domains.
#[cfg(feature = "cells")]
fn kept_frames(f: &FrameAllocator<'_>) -> u64 {
    reserve_frames(f) as u64 + RESERVE.frames()
}

/// Whether cells may take `bytes` more (frames or heap) and still leave the
/// kernel its reserve and reserved domains what is set aside for them.
#[cfg(feature = "cells")]
pub fn cells_may_take(bytes: u64) -> bool {
    with_frames(|f| f.free_frames() as u64 >= kept_frames(f) + bytes.div_ceil(FRAME_SIZE))
        .unwrap_or(false)
}

/// Allocates one frame kept for as long as the kernel runs or nearly (a
/// published page, design D21 of `add-cell-hierarchy`), keeping the
/// reserve and what is set aside for reserved domains: the highest free one, as the heap's, so frames that stay do not
/// split the free runs low in memory where cells allocate.
#[cfg(feature = "cells")]
pub fn alloc_kept() -> Option<u64> {
    with_frames(|f| {
        if (f.free_frames() as u64) < kept_frames(f) + 1 {
            return None;
        }
        f.alloc_high()
    })
    .flatten()
}

/// Allocates `count` contiguous frames for a cell or a cache, keeping the
/// reserve and what is set aside for reserved domains (a reserved cell's
/// meter took these frames out of it before asking).
#[cfg(feature = "cells")]
pub fn alloc_for_cell(count: usize) -> Option<u64> {
    with_frames(|f| {
        if (f.free_frames() as u64) < kept_frames(f) + count as u64 {
            return None;
        }
        match count {
            1 => f.alloc(),
            _ => f.alloc_large(),
        }
    })
    .flatten()
}

#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests; cells use it from H2")
)]
/// Translates a virtual address in the kernel address space.
pub fn translate(virt: u64) -> Option<Translation> {
    kernel_tables().translate(&DirectMap, virt)
}

#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests; cells use it from H2")
)]
/// Maps a 4 KiB page in the kernel address space.
pub fn map_page(virt: u64, phys: u64, flags: PageFlags) -> Result<(), MapError> {
    let pt = kernel_tables();
    with_frames(|frames| {
        let mut alloc = || frames.alloc();
        pt.map(
            &mut DirectMap,
            &mut alloc,
            virt,
            phys,
            PageSize::Small,
            flags,
        )
    })
    .unwrap_or(Err(MapError::OutOfFrames))
}

#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests; cells use it from H2")
)]
/// Unmaps a page in the kernel address space and returns its frame.
pub fn unmap_page(virt: u64) -> Result<u64, UnmapError> {
    kernel_tables()
        .unmap(&mut DirectMap, &mut CpuTlb, virt)
        .map(|(phys, _)| phys)
}
