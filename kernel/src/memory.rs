//! Kernel memory setup: memory map → frames → stage-2 page tables → heap.

use crate::arch::boot::layout;
use crate::arch::cpu::{self, CpuTlb};
use crate::sync::Global;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use rhizo_core::memory::frames::FrameAllocator;
use rhizo_core::memory::memmap::{BootInfo, RegionKind};
use rhizo_core::memory::paging::{
    MapError, PageFlags, PageSize, PageTable, Translation, UnmapError,
};
use rhizo_core::memory::{
    DIRECT_MAP_BASE, FRAME_SIZE, KERNEL_BASE, LARGE_PAGE_SIZE, MAX_PHYS, align_up,
};
use rhizo_hal::PhysicalMemory;

const ONE_MIB: u64 = 1024 * 1024;
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
        // SAFETY: the frame is RAM, mapped through the direct map, and owned
        // by the caller.
        unsafe { core::ptr::write_bytes(Self::virt(frame) as *mut u8, 0, FRAME_SIZE as usize) }
    }
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

/// Sets up physical memory and the kernel's final address space.
pub fn init(start_info: u64) -> Summary {
    let mut mem = DirectMap;
    let info = BootInfo::read(&mem, start_info)
        .unwrap_or_else(|e| panic!("memory map is missing or invalid: {e:?}"));

    // The bitmap is a zeroed static (all frames in use), so no clearing pass.
    let mut frames = FrameAllocator::from_zeroed(take_bitmap(), (MAX_PHYS / FRAME_SIZE) as usize);
    let mut top = 0;
    for region in info.regions(&mem) {
        if region.kind == RegionKind::Ram {
            let end = region.end().min(MAX_PHYS);
            frames.add_free_region(region.start.max(ONE_MIB), end);
            top = top.max(end);
        }
    }
    let (kernel_start, kernel_end) = layout::kernel_phys();
    frames.reserve(0, ONE_MIB);
    frames.reserve(kernel_start, kernel_end);
    for (start, end) in info.occupied() {
        frames.reserve(start, end);
    }
    let usable = frames.usable_frames() as u64 * FRAME_SIZE;

    let root = build_kernel_tables(&mut mem, &mut frames, top)
        .unwrap_or_else(|e| panic!("cannot build kernel page tables: {e:?}"));
    // SAFETY: the new tables map the kernel image (code, data, stack) at the
    // same virtual addresses and the direct map at the same offset.
    unsafe { cpu::write_cr3(root.root()) };
    KERNEL_ROOT.store(root.root(), Ordering::Release);

    FRAMES.with(|slot| *slot = Some(frames));
    Summary {
        usable,
        free: free_bytes(),
    }
}

fn build_kernel_tables(
    mem: &mut DirectMap,
    frames: &mut FrameAllocator<'static>,
    top: u64,
) -> Result<PageTable, MapError> {
    let mut alloc = || frames.alloc();
    let pt = PageTable::new(mem, &mut alloc)?;

    // Direct map of physical memory: 2 MiB pages, read/write, never executable.
    let rw_nx = PageFlags::WRITABLE | PageFlags::NO_EXECUTE;
    for phys in (0..align_up(top, LARGE_PAGE_SIZE)).step_by(LARGE_PAGE_SIZE as usize) {
        pt.map(
            mem,
            &mut alloc,
            DirectMap::virt(phys),
            phys,
            PageSize::Large,
            rw_nx,
        )?;
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
                    virt - KERNEL_BASE,
                    PageSize::Small,
                    flags,
                )?;
            }
        }
    }
    Ok(pt)
}

#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests; cells use it from H2")
)]
fn kernel_tables() -> PageTable {
    let root = KERNEL_ROOT.load(Ordering::Acquire);
    assert!(root != 0, "memory not initialized");
    PageTable::from_root(root)
}

/// Runs `f` with the frame allocator, or returns `None` before memory setup.
pub fn with_frames<R>(f: impl FnOnce(&mut FrameAllocator<'static>) -> R) -> Option<R> {
    FRAMES.with(|slot| slot.as_mut().map(f))
}

/// Bytes currently free in the frame allocator.
pub fn free_bytes() -> u64 {
    with_frames(|f| f.free_frames() as u64 * FRAME_SIZE).unwrap_or(0)
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
