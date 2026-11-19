//! Memory management for the Rhizo kernel: boot memory map, physical frames,
//! page tables and the kernel heap. Hardware access goes through `rhizo-hal`
//! only, so everything here is tested on the host.

#![cfg_attr(not(test), no_std)]

pub mod cow;
pub mod frames;
pub mod heap;
pub mod memmap;
pub mod paging;

/// Size of a physical frame and of a small page.
pub const FRAME_SIZE: u64 = 4096;

/// Size of a large (2 MiB) page.
pub const LARGE_PAGE_SIZE: u64 = 2 * 1024 * 1024;

/// Physical memory managed by the kernel ends here (exclusive): 4 GiB.
pub const MAX_PHYS: u64 = 4 * 1024 * 1024 * 1024;

/// Virtual address where the direct map of physical memory starts.
pub const DIRECT_MAP_BASE: u64 = 0xffff_8000_0000_0000;

/// Virtual address the kernel image is linked at (plus its physical address).
pub const KERNEL_BASE: u64 = 0xffff_ffff_8000_0000;

/// Rounds `value` down to a multiple of `align` (a power of two).
pub const fn align_down(value: u64, align: u64) -> u64 {
    value & !(align - 1)
}

/// Rounds `value` up to a multiple of `align` (a power of two), saturating.
pub const fn align_up(value: u64, align: u64) -> u64 {
    match value.checked_add(align - 1) {
        Some(v) => v & !(align - 1),
        None => !(align - 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_constants() {
        assert_eq!(MAX_PHYS, 1 << 32);
        assert_eq!(LARGE_PAGE_SIZE, 512 * FRAME_SIZE);
    }

    #[test]
    fn alignment_helpers() {
        assert_eq!(align_down(4097, 4096), 4096);
        assert_eq!(align_up(4097, 4096), 8192);
        assert_eq!(align_up(4096, 4096), 4096);
        assert_eq!(align_up(u64::MAX, 4096), !4095);
    }
}
