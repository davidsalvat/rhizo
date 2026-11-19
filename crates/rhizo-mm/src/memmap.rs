//! Boot memory map from the PVH `hvm_start_info` structure.
//!
//! Layout (Xen public `arch-x86/hvm/start_info.h`, version 1):
//!
//! | Offset | Field |
//! |---|---|
//! | 0 | `magic: u32`, 4 `version: u32` |
//! | 40 | `memmap_paddr: u64` |
//! | 48 | `memmap_entries: u32` |
//!
//! Each memory-map entry is 24 bytes: `addr: u64`, `size: u64`,
//! `type: u32`, `reserved: u32`. Type 1 is RAM.

use rhizo_hal::PhysicalMemory;

/// Magic value at the start of `hvm_start_info`.
pub const PVH_MAGIC: u32 = 0x336e_c578;

/// Upper bound on memory-map entries we accept.
pub const MAX_ENTRIES: usize = 128;

const ENTRY_SIZE: u64 = 24;
const TYPE_RAM: u32 = 1;

/// What a memory region is used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionKind {
    /// Usable RAM.
    Ram,
    /// Anything else (reserved, ACPI, MMIO…), identified by its raw type.
    Reserved(u32),
}

/// One entry of the memory map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryRegion {
    /// Physical start address.
    pub start: u64,
    /// Length in bytes.
    pub len: u64,
    /// What the region is.
    pub kind: RegionKind,
}

impl MemoryRegion {
    /// End address (exclusive), saturating at `u64::MAX`.
    pub fn end(&self) -> u64 {
        self.start.saturating_add(self.len)
    }
}

/// Why the boot information could not be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootInfoError {
    /// The structure does not start with the PVH magic.
    BadMagic(u32),
    /// The structure is older than version 1 and has no memory map.
    NoMemoryMap,
    /// The memory map has more entries than [`MAX_ENTRIES`].
    TooManyEntries(u32),
}

/// The parts of `hvm_start_info` the kernel uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootInfo {
    /// Physical address of the `hvm_start_info` structure.
    pub start_info: u64,
    /// Physical address of the memory map.
    pub memmap: u64,
    /// Number of memory-map entries.
    pub entries: usize,
}

impl BootInfo {
    /// Reads and validates `hvm_start_info` at physical address `start_info`.
    pub fn read(mem: &impl PhysicalMemory, start_info: u64) -> Result<Self, BootInfoError> {
        let head = mem.read_u64(start_info);
        let magic = head as u32;
        let version = (head >> 32) as u32;
        if magic != PVH_MAGIC {
            return Err(BootInfoError::BadMagic(magic));
        }
        if version < 1 {
            return Err(BootInfoError::NoMemoryMap);
        }
        let memmap = mem.read_u64(start_info + 40);
        let entries = mem.read_u64(start_info + 48) as u32;
        if memmap == 0 || entries == 0 {
            return Err(BootInfoError::NoMemoryMap);
        }
        if entries as usize > MAX_ENTRIES {
            return Err(BootInfoError::TooManyEntries(entries));
        }
        Ok(Self {
            start_info,
            memmap,
            entries: entries as usize,
        })
    }

    /// Returns memory-map entry `index` (must be below `self.entries`).
    pub fn region(&self, mem: &impl PhysicalMemory, index: usize) -> MemoryRegion {
        assert!(index < self.entries, "memory-map index out of range");
        let entry = self.memmap + index as u64 * ENTRY_SIZE;
        let raw_type = mem.read_u64(entry + 16) as u32;
        MemoryRegion {
            start: mem.read_u64(entry),
            len: mem.read_u64(entry + 8),
            kind: if raw_type == TYPE_RAM {
                RegionKind::Ram
            } else {
                RegionKind::Reserved(raw_type)
            },
        }
    }

    /// Iterates over every memory-map entry.
    pub fn regions<'m, M: PhysicalMemory>(
        &self,
        mem: &'m M,
    ) -> impl Iterator<Item = MemoryRegion> + 'm {
        let info = *self;
        (0..info.entries).map(move |i| info.region(mem, i))
    }

    /// Physical range `[start, end)` occupied by the boot information itself
    /// (the `hvm_start_info` structure and the memory map).
    pub fn occupied(&self) -> [(u64, u64); 2] {
        [
            (self.start_info, self.start_info + 56),
            (self.memmap, self.memmap + self.entries as u64 * ENTRY_SIZE),
        ]
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rhizo_hal::testing::FakePhysicalMemory;

    /// Writes a PVH start-info structure and memory map into fake memory.
    pub(crate) fn write_boot_info(
        mem: &mut FakePhysicalMemory,
        start_info: u64,
        memmap: u64,
        regions: &[(u64, u64, u32)],
    ) {
        mem.write_u64(start_info, (1u64 << 32) | PVH_MAGIC as u64);
        mem.write_u64(start_info + 40, memmap);
        mem.write_u64(start_info + 48, regions.len() as u64);
        for (i, &(addr, size, kind)) in regions.iter().enumerate() {
            let e = memmap + i as u64 * ENTRY_SIZE;
            mem.write_u64(e, addr);
            mem.write_u64(e + 8, size);
            mem.write_u64(e + 16, kind as u64);
        }
    }

    #[test]
    fn parses_ram_and_reserved_regions() {
        let mut mem = FakePhysicalMemory::new();
        write_boot_info(
            &mut mem,
            0x6000,
            0x7000,
            &[
                (0, 0x9fc00, 1),
                (0xf0000, 0x10000, 2),
                (0x100000, 0x7f00000, 1),
            ],
        );
        let info = BootInfo::read(&mem, 0x6000).unwrap();
        let regions: Vec<_> = info.regions(&mem).collect();
        assert_eq!(
            regions,
            vec![
                MemoryRegion {
                    start: 0,
                    len: 0x9fc00,
                    kind: RegionKind::Ram
                },
                MemoryRegion {
                    start: 0xf0000,
                    len: 0x10000,
                    kind: RegionKind::Reserved(2)
                },
                MemoryRegion {
                    start: 0x100000,
                    len: 0x7f00000,
                    kind: RegionKind::Ram
                },
            ]
        );
        assert_eq!(regions[2].end(), 0x8000000);
        assert_eq!(info.occupied(), [(0x6000, 0x6038), (0x7000, 0x7000 + 72)]);
    }

    #[test]
    fn rejects_bad_magic() {
        let mem = FakePhysicalMemory::new();
        assert_eq!(
            BootInfo::read(&mem, 0x6000),
            Err(BootInfoError::BadMagic(0))
        );
    }

    #[test]
    fn rejects_version_zero_and_empty_maps() {
        let mut mem = FakePhysicalMemory::new();
        mem.write_u64(0x6000, PVH_MAGIC as u64); // version 0
        assert_eq!(
            BootInfo::read(&mem, 0x6000),
            Err(BootInfoError::NoMemoryMap)
        );

        write_boot_info(&mut mem, 0x6000, 0x7000, &[]);
        assert_eq!(
            BootInfo::read(&mem, 0x6000),
            Err(BootInfoError::NoMemoryMap)
        );
    }

    #[test]
    fn accepts_newer_versions_and_exactly_the_maximum_entries() {
        let mut mem = FakePhysicalMemory::new();
        write_boot_info(&mut mem, 0x6000, 0x7000, &[(0, 0x1000, 1)]);
        mem.write_u64(0x6000, (2u64 << 32) | PVH_MAGIC as u64); // version 2
        mem.write_u64(0x6000 + 48, MAX_ENTRIES as u64);
        assert_eq!(BootInfo::read(&mem, 0x6000).unwrap().entries, MAX_ENTRIES);
    }

    #[test]
    fn caps_the_number_of_entries() {
        let mut mem = FakePhysicalMemory::new();
        write_boot_info(&mut mem, 0x6000, 0x7000, &[(0, 0x1000, 1)]);
        mem.write_u64(0x6000 + 48, MAX_ENTRIES as u64 + 1);
        assert_eq!(
            BootInfo::read(&mem, 0x6000),
            Err(BootInfoError::TooManyEntries(MAX_ENTRIES as u32 + 1))
        );
    }
}
