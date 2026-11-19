//! Four-level page tables with a 4 KiB granule and 48-bit virtual addresses.
//!
//! x86_64 (4-level paging) and AArch64 (4 KiB granule, 48-bit VA) share the
//! same shape: four levels of 512 eight-byte entries indexed by 9 bits of the
//! virtual address each. The walk is written once; only the encoding of an
//! entry differs, and that is what [`EntryFormat`] abstracts ([`X86_64`],
//! [`Aarch64`]).
//!
//! All accesses to page-table memory go through [`PhysicalMemory`], and TLB
//! invalidation through [`Tlb`], so the same code runs in the kernel and in
//! host tests.

use crate::{FRAME_SIZE, LARGE_PAGE_SIZE};
use core::marker::PhantomData;
use rhizo_hal::{PhysicalMemory, Tlb};

/// Architecture-neutral permissions and memory type of a mapping.
/// Every mapping is readable; flags add or remove rights from there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageFlags(u8);

impl PageFlags {
    /// Writes allowed.
    pub const WRITABLE: Self = Self(1 << 0);
    /// Accessible from user mode (cells).
    pub const USER: Self = Self(1 << 1);
    /// Instruction fetches not allowed.
    pub const NO_EXECUTE: Self = Self(1 << 2);
    /// Device memory (uncached, no speculation), for MMIO.
    pub const DEVICE: Self = Self(1 << 3);

    /// Read-only, executable, kernel-only, normal memory.
    pub const fn empty() -> Self {
        Self(0)
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
    /// 2 MiB page (block).
    Large,
    /// 1 GiB page (block; x86_64 needs CPU support for it).
    Huge,
}

impl PageSize {
    /// Size in bytes.
    pub const fn bytes(self) -> u64 {
        match self {
            PageSize::Small => FRAME_SIZE,
            PageSize::Large => LARGE_PAGE_SIZE,
            PageSize::Huge => HUGE_PAGE_SIZE,
        }
    }

    /// Level of the leaf entry (0 = last level, 1 = 2 MiB, 2 = 1 GiB).
    const fn leaf_level(self) -> u32 {
        match self {
            PageSize::Small => 0,
            PageSize::Large => 1,
            PageSize::Huge => 2,
        }
    }

    const fn from_leaf_level(level: u32) -> Self {
        match level {
            0 => PageSize::Small,
            1 => PageSize::Large,
            _ => PageSize::Huge,
        }
    }
}

/// Size of a huge (1 GiB) page.
pub const HUGE_PAGE_SIZE: u64 = 1024 * 1024 * 1024;

/// How one architecture encodes page-table entries.
pub trait EntryFormat {
    /// Entry pointing to the next-level table at `frame`.
    fn table(frame: u64) -> u64;
    /// Leaf entry mapping `phys` with `flags`, at the level of `size`.
    fn leaf(phys: u64, size: PageSize, flags: PageFlags) -> u64;
    /// Whether the entry is valid.
    fn is_valid(entry: u64) -> bool;
    /// Whether a valid entry at `level` maps memory (rather than a table).
    fn is_leaf(entry: u64, level: u32) -> bool;
    /// Output address of an entry (table frame or mapped memory).
    fn address(entry: u64) -> u64;
    /// Permissions of a leaf entry.
    fn flags(entry: u64) -> PageFlags;
}

/// x86_64 4-level paging entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct X86_64;

mod x86 {
    pub const PRESENT: u64 = 1 << 0;
    pub const WRITABLE: u64 = 1 << 1;
    pub const USER: u64 = 1 << 2;
    pub const WRITE_THROUGH: u64 = 1 << 3;
    pub const CACHE_DISABLE: u64 = 1 << 4;
    pub const HUGE: u64 = 1 << 7;
    pub const NO_EXECUTE: u64 = 1 << 63;
    pub const ADDR: u64 = 0x000f_ffff_ffff_f000;
}

impl EntryFormat for X86_64 {
    fn table(frame: u64) -> u64 {
        // Permissions are decided at the leaf.
        frame | x86::PRESENT | x86::WRITABLE | x86::USER
    }

    fn leaf(phys: u64, size: PageSize, flags: PageFlags) -> u64 {
        let mut e = phys | x86::PRESENT;
        if flags.contains(PageFlags::WRITABLE) {
            e |= x86::WRITABLE;
        }
        if flags.contains(PageFlags::USER) {
            e |= x86::USER;
        }
        if flags.contains(PageFlags::NO_EXECUTE) {
            e |= x86::NO_EXECUTE;
        }
        if flags.contains(PageFlags::DEVICE) {
            e |= x86::WRITE_THROUGH | x86::CACHE_DISABLE;
        }
        if size != PageSize::Small {
            e |= x86::HUGE;
        }
        e
    }

    fn is_valid(entry: u64) -> bool {
        entry & x86::PRESENT != 0
    }

    fn is_leaf(entry: u64, level: u32) -> bool {
        level == 0 || (level <= 2 && entry & x86::HUGE != 0)
    }

    fn address(entry: u64) -> u64 {
        entry & x86::ADDR
    }

    fn flags(entry: u64) -> PageFlags {
        let mut f = PageFlags::empty();
        if entry & x86::WRITABLE != 0 {
            f = f | PageFlags::WRITABLE;
        }
        if entry & x86::USER != 0 {
            f = f | PageFlags::USER;
        }
        if entry & x86::NO_EXECUTE != 0 {
            f = f | PageFlags::NO_EXECUTE;
        }
        if entry & x86::CACHE_DISABLE != 0 {
            f = f | PageFlags::DEVICE;
        }
        f
    }
}

/// AArch64 stage-1 translation entries (4 KiB granule, 48-bit VA).
///
/// Memory attributes index `MAIR_EL1`: index 0 must be normal write-back
/// memory and index 1 device nGnRE (the kernel programs it that way).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aarch64;

mod a64 {
    pub const VALID: u64 = 1 << 0;
    /// Bit 1: table (levels 1-3 of the hardware) or page (last level).
    pub const TABLE_OR_PAGE: u64 = 1 << 1;
    pub const ATTR_NORMAL: u64 = 0 << 2;
    pub const ATTR_DEVICE: u64 = 1 << 2;
    pub const ATTR_MASK: u64 = 0b111 << 2;
    /// AP[1]: accessible from EL0.
    pub const AP_EL0: u64 = 1 << 6;
    /// AP[2]: read-only.
    pub const AP_READ_ONLY: u64 = 1 << 7;
    pub const INNER_SHAREABLE: u64 = 0b11 << 8;
    pub const ACCESS_FLAG: u64 = 1 << 10;
    pub const NOT_GLOBAL: u64 = 1 << 11;
    /// Privileged (EL1) execute-never.
    pub const PXN: u64 = 1 << 53;
    /// Unprivileged (EL0) execute-never.
    pub const UXN: u64 = 1 << 54;
    pub const ADDR: u64 = 0x0000_ffff_ffff_f000;
}

impl EntryFormat for Aarch64 {
    fn table(frame: u64) -> u64 {
        frame | a64::VALID | a64::TABLE_OR_PAGE
    }

    fn leaf(phys: u64, size: PageSize, flags: PageFlags) -> u64 {
        let mut e = phys | a64::VALID | a64::ACCESS_FLAG;
        if size == PageSize::Small {
            e |= a64::TABLE_OR_PAGE; // page descriptor; blocks leave bit 1 clear
        }
        if flags.contains(PageFlags::DEVICE) {
            e |= a64::ATTR_DEVICE;
        } else {
            e |= a64::ATTR_NORMAL | a64::INNER_SHAREABLE;
        }
        if !flags.contains(PageFlags::WRITABLE) {
            e |= a64::AP_READ_ONLY;
        }
        if flags.contains(PageFlags::USER) {
            // Cell memory: per-cell TLB entries (ASID) and never executable
            // by the kernel.
            e |= a64::AP_EL0 | a64::NOT_GLOBAL | a64::PXN;
            if flags.contains(PageFlags::NO_EXECUTE) {
                e |= a64::UXN;
            }
        } else {
            // Kernel memory: never executable from EL0.
            e |= a64::UXN;
            if flags.contains(PageFlags::NO_EXECUTE) {
                e |= a64::PXN;
            }
        }
        e
    }

    fn is_valid(entry: u64) -> bool {
        entry & a64::VALID != 0
    }

    fn is_leaf(entry: u64, level: u32) -> bool {
        // Last level: always a page. Levels 1-2 (2 MiB, 1 GiB): block if bit 1
        // is clear. The top level holds only tables.
        level == 0 || (level <= 2 && entry & a64::TABLE_OR_PAGE == 0)
    }

    fn address(entry: u64) -> u64 {
        entry & a64::ADDR
    }

    fn flags(entry: u64) -> PageFlags {
        let mut f = PageFlags::empty();
        let user = entry & a64::AP_EL0 != 0;
        if entry & a64::AP_READ_ONLY == 0 {
            f = f | PageFlags::WRITABLE;
        }
        if user {
            f = f | PageFlags::USER;
        }
        let xn = if user { a64::UXN } else { a64::PXN };
        if entry & xn != 0 {
            f = f | PageFlags::NO_EXECUTE;
        }
        if entry & a64::ATTR_MASK == a64::ATTR_DEVICE {
            f = f | PageFlags::DEVICE;
        }
        f
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
    /// Permissions of the leaf entry.
    pub flags: PageFlags,
    /// Size of the page containing the address.
    pub size: PageSize,
}

const ENTRIES: u64 = 512;
/// Physical addresses must fit in 48 bits (both architectures, as used here).
const PHYS_LIMIT: u64 = 1 << 48;

/// Whether `virt` is a canonical 48-bit address (top 16 bits all equal to
/// bit 47; on AArch64 this selects `TTBR0` or `TTBR1`).
pub const fn is_canonical(virt: u64) -> bool {
    let top = virt >> 47;
    top == 0 || top == 0x1_ffff
}

const fn index(virt: u64, level: u32) -> u64 {
    (virt >> (12 + 9 * level)) & (ENTRIES - 1)
}

/// A four-level page-table hierarchy in format `F`, identified by its root
/// frame (the value for `CR3` or `TTBRn_EL1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageTable<F> {
    root: u64,
    format: PhantomData<F>,
}

impl<F: EntryFormat> PageTable<F> {
    /// Creates an empty hierarchy using a zeroed frame from `alloc_frame`.
    pub fn new(
        mem: &mut impl PhysicalMemory,
        alloc_frame: &mut impl FnMut() -> Option<u64>,
    ) -> Result<Self, MapError> {
        let root = alloc_frame().ok_or(MapError::OutOfFrames)?;
        mem.zero_frame(root);
        Ok(Self::from_root(root))
    }

    /// Wraps an existing hierarchy rooted at `root`.
    pub const fn from_root(root: u64) -> Self {
        Self {
            root,
            format: PhantomData,
        }
    }

    /// Physical address of the root table.
    pub const fn root(&self) -> u64 {
        self.root
    }

    /// Maps the page at `virt` to `phys` with `flags`.
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
            || phys >= PHYS_LIMIT
        {
            return Err(MapError::Misaligned);
        }
        let leaf_level = size.leaf_level();

        let mut table = self.root;
        for level in (leaf_level + 1..=3).rev() {
            let slot = table + index(virt, level) * 8;
            let entry = mem.read_u64(slot);
            table = if !F::is_valid(entry) {
                let frame = alloc_frame().ok_or(MapError::OutOfFrames)?;
                mem.zero_frame(frame);
                mem.write_u64(slot, F::table(frame));
                frame
            } else if F::is_leaf(entry, level) {
                return Err(MapError::AlreadyMapped(virt));
            } else {
                F::address(entry)
            };
        }

        let slot = table + index(virt, leaf_level) * 8;
        if F::is_valid(mem.read_u64(slot)) {
            return Err(MapError::AlreadyMapped(virt));
        }
        mem.write_u64(slot, F::leaf(phys, size, flags));
        Ok(())
    }

    /// Returns the physical address and flags `virt` maps to, if any.
    pub fn translate(&self, mem: &impl PhysicalMemory, virt: u64) -> Option<Translation> {
        let (_, entry, level) = self.leaf_slot(mem, virt)?;
        let size = PageSize::from_leaf_level(level);
        let offset = virt & (size.bytes() - 1);
        let base = F::address(entry) & !(size.bytes() - 1);
        Some(Translation {
            phys: base + offset,
            flags: F::flags(entry),
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
        let size = PageSize::from_leaf_level(level);
        mem.write_u64(slot, 0);
        tlb.flush_page(virt & !(size.bytes() - 1));
        Ok((F::address(entry) & !(size.bytes() - 1), size))
    }

    /// Copies the root entries of the upper (kernel) half from `kernel`, so
    /// this hierarchy shares the kernel's mappings (x86_64 cells).
    pub fn share_upper_half(&self, mem: &mut impl PhysicalMemory, kernel: &PageTable<F>) {
        for i in ENTRIES / 2..ENTRIES {
            mem.write_u64(self.root + i * 8, mem.read_u64(kernel.root + i * 8));
        }
    }

    /// Number of table frames used by the lower (user) half, root included.
    pub fn lower_half_tables(&self, mem: &impl PhysicalMemory) -> usize {
        fn walk<F: EntryFormat>(
            mem: &impl PhysicalMemory,
            table: u64,
            level: u32,
            range: core::ops::Range<u64>,
        ) -> usize {
            range
                .map(|i| mem.read_u64(table + i * 8))
                .filter(|&e| F::is_valid(e) && !F::is_leaf(e, level))
                .map(|e| 1 + walk::<F>(mem, F::address(e), level - 1, 0..ENTRIES))
                .sum()
        }
        1 + walk::<F>(mem, self.root, 3, 0..ENTRIES / 2)
    }

    /// Frees everything mapped in the lower (user) half and every table frame
    /// it used, then the root: `free(phys, size)` is called once per mapped
    /// page and once per table frame (as `PageSize::Small`). The upper half is
    /// left alone (it is shared with the kernel).
    pub fn destroy_lower_half(
        self,
        mem: &mut impl PhysicalMemory,
        free: &mut impl FnMut(u64, PageSize),
    ) {
        fn walk<F: EntryFormat>(
            mem: &mut impl PhysicalMemory,
            table: u64,
            level: u32,
            range: core::ops::Range<u64>,
            free: &mut impl FnMut(u64, PageSize),
        ) {
            for i in range {
                let entry = mem.read_u64(table + i * 8);
                if !F::is_valid(entry) {
                    continue;
                }
                if F::is_leaf(entry, level) {
                    let size = PageSize::from_leaf_level(level);
                    free(F::address(entry) & !(size.bytes() - 1), size);
                } else {
                    let next = F::address(entry);
                    walk::<F>(mem, next, level - 1, 0..ENTRIES, free);
                    free(next, PageSize::Small);
                }
            }
        }
        walk::<F>(mem, self.root, 3, 0..ENTRIES / 2, free);
        free(self.root, PageSize::Small);
    }

    /// Walks to the valid leaf entry for `virt`: (slot address, entry, level).
    fn leaf_slot(&self, mem: &impl PhysicalMemory, virt: u64) -> Option<(u64, u64, u32)> {
        if !is_canonical(virt) {
            return None;
        }
        let mut table = self.root;
        for level in (0..=3).rev() {
            let slot = table + index(virt, level) * 8;
            let entry = mem.read_u64(slot);
            if !F::is_valid(entry) {
                return None;
            }
            if F::is_leaf(entry, level) {
                return Some((slot, entry, level));
            }
            table = F::address(entry);
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

    /// The same behavior tests run for every entry format.
    macro_rules! format_tests {
        ($module:ident, $format:ty) => {
            mod $module {
                use super::*;
                type PT = PageTable<$format>;

                #[test]
                fn map_translate_unmap_small_page() {
                    let mut mem = FakePhysicalMemory::new();
                    let mut alloc = frames();
                    let pt = PT::new(&mut mem, &mut alloc).unwrap();
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
                    let pt = PT::new(&mut mem, &mut alloc).unwrap();
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
                }

                #[test]
                fn huge_pages_translate_and_unmap() {
                    let mut mem = FakePhysicalMemory::new();
                    let mut alloc = frames();
                    let pt = PT::new(&mut mem, &mut alloc).unwrap();
                    let virt = 0xffff_8000_4000_0000;
                    pt.map(
                        &mut mem,
                        &mut alloc,
                        virt,
                        HUGE_PAGE_SIZE,
                        PageSize::Huge,
                        RW_NX,
                    )
                    .unwrap();
                    let t = pt.translate(&mem, virt + 0x1234_5678).unwrap();
                    assert_eq!(t.phys, HUGE_PAGE_SIZE + 0x1234_5678);
                    assert_eq!(t.size, PageSize::Huge);
                    assert!(t.flags.is_writable() && !t.flags.is_executable());

                    // Nothing smaller can be mapped inside it, and it needs no PD or PT.
                    assert_eq!(
                        pt.map(
                            &mut mem,
                            &mut alloc,
                            virt + 0x20_0000,
                            0,
                            PageSize::Large,
                            RW_NX
                        ),
                        Err(MapError::AlreadyMapped(virt + 0x20_0000))
                    );
                    let mut tlb = RecordingTlb::default();
                    assert_eq!(
                        pt.unmap(&mut mem, &mut tlb, virt + 0x1000),
                        Ok((HUGE_PAGE_SIZE, PageSize::Huge))
                    );
                    assert_eq!(tlb.flushed, vec![virt]);
                    assert_eq!(
                        pt.map(&mut mem, &mut alloc, virt, 0x1000, PageSize::Huge, RW_NX),
                        Err(MapError::Misaligned)
                    );
                }

                #[test]
                fn huge_page_needs_only_root_and_pdpt() {
                    let mut mem = FakePhysicalMemory::new();
                    let mut count = 0;
                    let mut inner = frames();
                    let mut alloc = || {
                        count += 1;
                        inner()
                    };
                    let pt = PT::new(&mut mem, &mut alloc).unwrap();
                    pt.map(
                        &mut mem,
                        &mut alloc,
                        0xffff_8000_0000_0000,
                        0,
                        PageSize::Huge,
                        RW_NX,
                    )
                    .unwrap();
                    assert_eq!(count, 2);
                }

                #[test]
                fn executable_read_only_mapping() {
                    let mut mem = FakePhysicalMemory::new();
                    let mut alloc = frames();
                    let pt = PT::new(&mut mem, &mut alloc).unwrap();
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
                    let pt = PT::new(&mut mem, &mut alloc).unwrap();
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
                    let pt = PT::new(&mut mem, &mut alloc).unwrap();
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
                    let pt = PT::new(&mut mem, &mut alloc).unwrap();
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
                    let pt = PT::new(&mut mem, &mut alloc).unwrap();
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
                fn destroying_the_lower_half_frees_every_frame_once() {
                    let mut mem = FakePhysicalMemory::new();
                    let mut alloc = frames();
                    let kernel = PT::new(&mut mem, &mut alloc).unwrap();
                    kernel
                        .map(
                            &mut mem,
                            &mut alloc,
                            0xffff_8000_0000_0000,
                            0,
                            PageSize::Huge,
                            RW_NX,
                        )
                        .unwrap();
                    let cell = PT::new(&mut mem, &mut alloc).unwrap();
                    cell.share_upper_half(&mut mem, &kernel);
                    let user = PageFlags::USER | PageFlags::WRITABLE | PageFlags::NO_EXECUTE;
                    cell.map(
                        &mut mem,
                        &mut alloc,
                        0x40_0000,
                        0x900_0000,
                        PageSize::Small,
                        user,
                    )
                    .unwrap();
                    cell.map(
                        &mut mem,
                        &mut alloc,
                        0x4000_0000_0000,
                        0xa00_0000,
                        PageSize::Large,
                        user,
                    )
                    .unwrap();
                    assert!(
                        cell.translate(&mem, 0xffff_8000_0000_1000).is_some(),
                        "kernel half shared"
                    );
                    // Root + three levels for the 4 KiB page + two for the 2 MiB
                    // one; the shared kernel half does not count.
                    assert_eq!(cell.lower_half_tables(&mem), 6);

                    let mut freed = Vec::new();
                    cell.destroy_lower_half(&mut mem, &mut |phys, size| freed.push((phys, size)));
                    // Both user pages, every table frame of the cell, then its root.
                    assert!(freed.contains(&(0x900_0000, PageSize::Small)));
                    assert!(freed.contains(&(0xa00_0000, PageSize::Large)));
                    assert_eq!(freed.last(), Some(&(cell.root(), PageSize::Small)));
                    let mut seen = std::collections::HashSet::new();
                    assert!(
                        freed.iter().all(|f| seen.insert(f.0)),
                        "nothing freed twice"
                    );
                    // The kernel's tables are untouched.
                    assert!(!freed.iter().any(|&(p, _)| p == kernel.root()));
                    assert!(kernel.translate(&mem, 0xffff_8000_0000_1000).is_some());
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
        };
    }

    format_tests!(x86_64, X86_64);
    format_tests!(aarch64, Aarch64);

    #[test]
    fn x86_64_encoding() {
        let rw_nx = PageFlags::WRITABLE | PageFlags::NO_EXECUTE;
        assert_eq!(
            X86_64::leaf(0x5000, PageSize::Small, rw_nx),
            0x5000 | 0b11 | 1 << 63
        );
        assert_eq!(
            X86_64::leaf(0x20_0000, PageSize::Large, PageFlags::empty()),
            0x20_0000 | 1 | 1 << 7
        );
        assert_eq!(X86_64::table(0x9000), 0x9000 | 0b111);
        assert!(!X86_64::is_leaf(X86_64::table(0x9000), 2));
    }

    #[test]
    fn aarch64_encoding() {
        const AF: u64 = 1 << 10;
        const SH: u64 = 0b11 << 8;
        const RO: u64 = 1 << 7;
        const EL0: u64 = 1 << 6;
        const NG: u64 = 1 << 11;
        const PXN: u64 = 1 << 53;
        const UXN: u64 = 1 << 54;
        // Kernel code: read-only, executable at EL1 only.
        assert_eq!(
            Aarch64::leaf(0x4000_0000, PageSize::Small, PageFlags::empty()),
            0x4000_0000 | 0b11 | AF | SH | RO | UXN
        );
        // Direct map: 1 GiB block, read/write, never executable.
        let rw_nx = PageFlags::WRITABLE | PageFlags::NO_EXECUTE;
        assert_eq!(
            Aarch64::leaf(0x4000_0000, PageSize::Huge, rw_nx),
            0x4000_0000 | 0b01 | AF | SH | UXN | PXN
        );
        // UART: device memory, attribute index 1, not shareable-normal.
        assert_eq!(
            Aarch64::leaf(
                0x0900_0000,
                PageSize::Small,
                PageFlags::WRITABLE | PageFlags::DEVICE | PageFlags::NO_EXECUTE
            ),
            0x0900_0000 | 0b11 | AF | 1 << 2 | UXN | PXN
        );
        // Cell memory: EL0 access, non-global (ASID-tagged), never kernel-executable.
        assert_eq!(
            Aarch64::leaf(
                0x1000,
                PageSize::Small,
                PageFlags::USER | PageFlags::WRITABLE
            ),
            0x1000 | 0b11 | AF | SH | EL0 | NG | PXN
        );
        assert_eq!(Aarch64::table(0x9000), 0x9000 | 0b11);
        assert!(!Aarch64::is_leaf(Aarch64::table(0x9000), 1));
        assert!(Aarch64::is_leaf(
            Aarch64::leaf(0, PageSize::Large, rw_nx),
            1
        ));
    }

    #[test]
    fn flags_round_trip_through_both_encodings() {
        use PageFlags as F;
        let all = [
            F::empty(),
            F::WRITABLE,
            F::NO_EXECUTE,
            F::WRITABLE | F::NO_EXECUTE,
            F::USER,
            F::USER | F::WRITABLE | F::NO_EXECUTE,
            F::DEVICE | F::WRITABLE | F::NO_EXECUTE,
        ];
        for flags in all {
            for size in [PageSize::Small, PageSize::Large, PageSize::Huge] {
                assert_eq!(
                    X86_64::flags(X86_64::leaf(0, size, flags)),
                    flags,
                    "x86_64 {flags:?}"
                );
                assert_eq!(
                    Aarch64::flags(Aarch64::leaf(0, size, flags)),
                    flags,
                    "aarch64 {flags:?}"
                );
            }
        }
    }
}
