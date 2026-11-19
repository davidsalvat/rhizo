//! x86_64 four-level page tables.
//!
//! All accesses to page-table memory go through [`PhysicalMemory`], and TLB
//! invalidation through [`Tlb`], so the same code runs in the kernel and in
//! host tests.

use super::{FRAME_SIZE, LARGE_PAGE_SIZE};
use rhizo_hal::{PhysicalMemory, Tlb};

/// Page-table entry flags (subset used by Rhizo).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageFlags(u64);

impl PageFlags {
    /// Entry is valid.
    pub const PRESENT: Self = Self(1 << 0);
    /// Writes allowed.
    pub const WRITABLE: Self = Self(1 << 1);
    /// Accessible from user mode.
    pub const USER: Self = Self(1 << 2);
    /// Entry maps a large page (2 MiB at the PD level).
    pub const HUGE: Self = Self(1 << 7);
    /// Instruction fetches not allowed (requires `EFER.NXE`).
    pub const NO_EXECUTE: Self = Self(1 << 63);

    /// No flags.
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Raw bits.
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Union of two flag sets.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether every flag in `other` is set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether the mapping allows writes.
    pub const fn is_writable(self) -> bool {
        self.contains(Self::WRITABLE)
    }

    /// Whether the mapping allows instruction fetches.
    pub const fn is_executable(self) -> bool {
        !self.contains(Self::NO_EXECUTE)
    }
}

impl core::ops::BitOr for PageFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

/// Size of a mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageSize {
    /// 4 KiB page.
    Small,
    /// 2 MiB page.
    Large,
}

impl PageSize {
    /// Size in bytes.
    pub const fn bytes(self) -> u64 {
        match self {
            PageSize::Small => FRAME_SIZE,
            PageSize::Large => LARGE_PAGE_SIZE,
        }
    }
}

/// Why a mapping could not be created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    /// The virtual address is not canonical.
    NonCanonical(u64),
    /// Virtual or physical address not aligned to the page size.
    Misaligned,
    /// Something is already mapped there.
    AlreadyMapped(u64),
    /// No frame was available for an intermediate table.
    OutOfFrames,
}

/// Why a mapping could not be removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnmapError {
    /// Nothing is mapped at that address.
    NotMapped(u64),
}

/// Result of a translation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Translation {
    /// Physical address the virtual address maps to.
    pub phys: u64,
    /// Flags of the leaf entry.
    pub flags: PageFlags,
    /// Size of the page containing the address.
    pub size: PageSize,
}

const ENTRIES: u64 = 512;
const ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;
const FLAG_MASK: u64 = !ADDR_MASK;
/// Flags for intermediate tables: permissions are decided at the leaf.
const TABLE_FLAGS: u64 = PageFlags::PRESENT.0 | PageFlags::WRITABLE.0 | PageFlags::USER.0;

/// Whether `virt` is a canonical 48-bit address.
pub const fn is_canonical(virt: u64) -> bool {
    let top = virt >> 47;
    top == 0 || top == 0x1_ffff
}

const fn index(virt: u64, level: u32) -> u64 {
    (virt >> (12 + 9 * level)) & (ENTRIES - 1)
}

/// A four-level page-table hierarchy, identified by its root (PML4) frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageTable {
    root: u64,
}

impl PageTable {
    /// Creates an empty hierarchy using a zeroed frame from `alloc_frame`.
    pub fn new(
        mem: &mut impl PhysicalMemory,
        alloc_frame: &mut impl FnMut() -> Option<u64>,
    ) -> Result<Self, MapError> {
        let root = alloc_frame().ok_or(MapError::OutOfFrames)?;
        mem.zero_frame(root);
        Ok(Self { root })
    }

    /// Wraps an existing hierarchy rooted at `root`.
    pub const fn from_root(root: u64) -> Self {
        Self { root }
    }

    /// Physical address of the root table (the value for `CR3`).
    pub const fn root(&self) -> u64 {
        self.root
    }

    /// Maps the page at `virt` to `phys`. `PRESENT` is added automatically.
    ///
    /// Intermediate tables are allocated (zeroed) with `alloc_frame`. Fails
    /// without changing any existing entry if anything is already mapped
    /// where the new page would go.
    pub fn map(
        &self,
        mem: &mut impl PhysicalMemory,
        alloc_frame: &mut impl FnMut() -> Option<u64>,
        virt: u64,
        phys: u64,
        size: PageSize,
        flags: PageFlags,
    ) -> Result<(), MapError> {
        if !is_canonical(virt) {
            return Err(MapError::NonCanonical(virt));
        }
        if !virt.is_multiple_of(size.bytes())
            || !phys.is_multiple_of(size.bytes())
            || phys & !ADDR_MASK != 0
        {
            return Err(MapError::Misaligned);
        }
        let leaf_level = match size {
            PageSize::Small => 0,
            PageSize::Large => 1,
        };

        let mut table = self.root;
        for level in (leaf_level + 1..=3).rev() {
            let slot = table + index(virt, level) * 8;
            let entry = mem.read_u64(slot);
            table = if entry & PageFlags::PRESENT.0 == 0 {
                let frame = alloc_frame().ok_or(MapError::OutOfFrames)?;
                mem.zero_frame(frame);
                mem.write_u64(slot, frame | TABLE_FLAGS);
                frame
            } else if entry & PageFlags::HUGE.0 != 0 {
                return Err(MapError::AlreadyMapped(virt));
            } else {
                entry & ADDR_MASK
            };
        }

        let slot = table + index(virt, leaf_level) * 8;
        if mem.read_u64(slot) & PageFlags::PRESENT.0 != 0 {
            return Err(MapError::AlreadyMapped(virt));
        }
        let mut bits = phys | flags.0 | PageFlags::PRESENT.0;
        if size == PageSize::Large {
            bits |= PageFlags::HUGE.0;
        }
        mem.write_u64(slot, bits);
        Ok(())
    }

    /// Returns the physical address and flags `virt` maps to, if any.
    pub fn translate(&self, mem: &impl PhysicalMemory, virt: u64) -> Option<Translation> {
        let (entry, level) = self.leaf(mem, virt)?;
        let size = if level == 1 {
            PageSize::Large
        } else {
            PageSize::Small
        };
        let offset = virt & (size.bytes() - 1);
        let base = entry & ADDR_MASK & !(size.bytes() - 1);
        Some(Translation {
            phys: base + offset,
            flags: PageFlags(entry & FLAG_MASK & !PageFlags::HUGE.0),
            size,
        })
    }

    /// Removes the mapping of the page containing `virt`, invalidates its TLB
    /// entry, and returns the physical page and its size.
    pub fn unmap(
        &self,
        mem: &mut impl PhysicalMemory,
        tlb: &mut impl Tlb,
        virt: u64,
    ) -> Result<(u64, PageSize), UnmapError> {
        let (slot, entry, level) = self
            .leaf_slot(mem, virt)
            .ok_or(UnmapError::NotMapped(virt))?;
        let size = if level == 1 {
            PageSize::Large
        } else {
            PageSize::Small
        };
        mem.write_u64(slot, 0);
        tlb.flush_page(virt & !(size.bytes() - 1));
        Ok((entry & ADDR_MASK & !(size.bytes() - 1), size))
    }

    fn leaf(&self, mem: &impl PhysicalMemory, virt: u64) -> Option<(u64, u32)> {
        self.leaf_slot(mem, virt)
            .map(|(_, entry, level)| (entry, level))
    }

    /// Walks to the present leaf entry for `virt`: (slot address, entry, level).
    fn leaf_slot(&self, mem: &impl PhysicalMemory, virt: u64) -> Option<(u64, u64, u32)> {
        if !is_canonical(virt) {
            return None;
        }
        let mut table = self.root;
        for level in (0..=3).rev() {
            let slot = table + index(virt, level) * 8;
            let entry = mem.read_u64(slot);
            if entry & PageFlags::PRESENT.0 == 0 {
                return None;
            }
            if level == 0 || (level == 1 && entry & PageFlags::HUGE.0 != 0) {
                return Some((slot, entry, level));
            }
            if entry & PageFlags::HUGE.0 != 0 {
                // 1 GiB pages are not created by Rhizo.
                return None;
            }
            table = entry & ADDR_MASK;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhizo_hal::testing::{FakePhysicalMemory, RecordingTlb};

    /// Hands out frames from 16 MiB upwards.
    fn frames() -> impl FnMut() -> Option<u64> {
        let mut next = 16 * 1024 * 1024;
        move || {
            let f = next;
            next += FRAME_SIZE;
            Some(f)
        }
    }

    const RW_NX: PageFlags = PageFlags::WRITABLE.union(PageFlags::NO_EXECUTE);

    #[test]
    fn map_translate_unmap_small_page() {
        let mut mem = FakePhysicalMemory::new();
        let mut alloc = frames();
        let pt = PageTable::new(&mut mem, &mut alloc).unwrap();
        let virt = 0xffff_c000_0012_3000;

        pt.map(&mut mem, &mut alloc, virt, 0x5000, PageSize::Small, RW_NX)
            .unwrap();
        let t = pt.translate(&mem, virt + 0x45).unwrap();
        assert_eq!(t.phys, 0x5045);
        assert_eq!(t.size, PageSize::Small);
        assert!(t.flags.is_writable());
        assert!(!t.flags.is_executable());

        let mut tlb = RecordingTlb::default();
        assert_eq!(
            pt.unmap(&mut mem, &mut tlb, virt + 8),
            Ok((0x5000, PageSize::Small))
        );
        assert_eq!(tlb.flushed, vec![virt]);
        assert_eq!(pt.translate(&mem, virt), None);
        assert_eq!(
            pt.unmap(&mut mem, &mut tlb, virt),
            Err(UnmapError::NotMapped(virt))
        );
    }

    #[test]
    fn large_pages_translate_with_offset() {
        let mut mem = FakePhysicalMemory::new();
        let mut alloc = frames();
        let pt = PageTable::new(&mut mem, &mut alloc).unwrap();
        let virt = 0xffff_8000_0020_0000;
        pt.map(
            &mut mem,
            &mut alloc,
            virt,
            0x20_0000,
            PageSize::Large,
            RW_NX,
        )
        .unwrap();
        let t = pt.translate(&mem, virt + 0x1_2345).unwrap();
        assert_eq!(t.phys, 0x21_2345);
        assert_eq!(t.size, PageSize::Large);
        assert!(!t.flags.contains(PageFlags::HUGE), "HUGE is internal");
    }

    #[test]
    fn executable_read_only_mapping() {
        let mut mem = FakePhysicalMemory::new();
        let mut alloc = frames();
        let pt = PageTable::new(&mut mem, &mut alloc).unwrap();
        pt.map(
            &mut mem,
            &mut alloc,
            0x40_0000,
            0x7000,
            PageSize::Small,
            PageFlags::empty(),
        )
        .unwrap();
        let t = pt.translate(&mem, 0x40_0000).unwrap();
        assert!(t.flags.is_executable());
        assert!(!t.flags.is_writable());
    }

    #[test]
    fn double_mapping_is_rejected_and_keeps_the_original() {
        let mut mem = FakePhysicalMemory::new();
        let mut alloc = frames();
        let pt = PageTable::new(&mut mem, &mut alloc).unwrap();
        pt.map(&mut mem, &mut alloc, 0x1000, 0x5000, PageSize::Small, RW_NX)
            .unwrap();
        assert_eq!(
            pt.map(&mut mem, &mut alloc, 0x1000, 0x9000, PageSize::Small, RW_NX),
            Err(MapError::AlreadyMapped(0x1000))
        );
        assert_eq!(pt.translate(&mem, 0x1000).unwrap().phys, 0x5000);

        // A small page cannot be placed inside an existing large page.
        pt.map(
            &mut mem,
            &mut alloc,
            0x40_0000,
            0x40_0000,
            PageSize::Large,
            RW_NX,
        )
        .unwrap();
        assert_eq!(
            pt.map(
                &mut mem,
                &mut alloc,
                0x40_1000,
                0x9000,
                PageSize::Small,
                RW_NX
            ),
            Err(MapError::AlreadyMapped(0x40_1000))
        );
    }

    #[test]
    fn non_canonical_and_misaligned_requests_touch_nothing() {
        let mut mem = FakePhysicalMemory::new();
        let mut alloc = frames();
        let pt = PageTable::new(&mut mem, &mut alloc).unwrap();
        let before = mem.nonzero_words();
        assert_eq!(
            pt.map(
                &mut mem,
                &mut alloc,
                0x0000_8000_0000_0000,
                0x5000,
                PageSize::Small,
                RW_NX
            ),
            Err(MapError::NonCanonical(0x0000_8000_0000_0000))
        );
        assert_eq!(
            pt.map(&mut mem, &mut alloc, 0x1001, 0x5000, PageSize::Small, RW_NX),
            Err(MapError::Misaligned)
        );
        assert_eq!(
            pt.map(
                &mut mem,
                &mut alloc,
                0x20_0000,
                0x1000,
                PageSize::Large,
                RW_NX
            ),
            Err(MapError::Misaligned)
        );
        assert_eq!(mem.nonzero_words(), before);
        assert_eq!(pt.translate(&mem, 0x0000_8000_0000_0000), None);
    }

    #[test]
    fn out_of_frames_is_reported() {
        let mut mem = FakePhysicalMemory::new();
        let mut one = Some(0x10_0000);
        let mut alloc = || one.take();
        let pt = PageTable::new(&mut mem, &mut alloc).unwrap();
        assert_eq!(
            pt.map(&mut mem, &mut alloc, 0x1000, 0x5000, PageSize::Small, RW_NX),
            Err(MapError::OutOfFrames)
        );
    }

    #[test]
    fn intermediate_tables_are_shared() {
        let mut mem = FakePhysicalMemory::new();
        let mut count = 0;
        let mut inner = frames();
        let mut alloc = || {
            count += 1;
            inner()
        };
        let pt = PageTable::new(&mut mem, &mut alloc).unwrap();
        for i in 0..8 {
            pt.map(
                &mut mem,
                &mut alloc,
                0x1000 * i,
                0x1000 * i,
                PageSize::Small,
                RW_NX,
            )
            .unwrap();
        }
        // root + PDPT + PD + PT, regardless of how many pages share them.
        assert_eq!(count, 4);
    }

    #[test]
    fn canonical_addresses() {
        assert!(is_canonical(0));
        assert!(is_canonical(0x0000_7fff_ffff_ffff));
        assert!(!is_canonical(0x0000_8000_0000_0000));
        assert!(!is_canonical(0xffff_7fff_ffff_ffff));
        assert!(is_canonical(0xffff_8000_0000_0000));
        assert!(is_canonical(u64::MAX));
    }
}
