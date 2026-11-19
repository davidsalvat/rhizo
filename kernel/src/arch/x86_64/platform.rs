//! What the platform tells the kernel at boot (PVH `hvm_start_info`).

use crate::memory::DirectMap;
use rhizo_mm::memmap::{BootInfo, RegionKind};
use rhizo_mm::paging::{MapError, PageTable};

/// Page-table entry format of this architecture.
pub type Format = rhizo_mm::paging::X86_64;

const ONE_MIB: u64 = 1024 * 1024;

fn boot_info() -> BootInfo {
    BootInfo::read(&DirectMap, super::boot::boot_arg())
        .unwrap_or_else(|e| panic!("memory map is missing or invalid: {e:?}"))
}

/// The kernel command line, copied into `buf`.
pub fn cmdline(buf: &mut [u8]) -> &[u8] {
    rhizo_core::options::read_cmdline(&DirectMap, super::boot::boot_arg(), buf)
}

/// Calls `f(start, end)` for every RAM region.
pub fn for_each_ram(mut f: impl FnMut(u64, u64)) {
    let info = boot_info();
    for region in info.regions(&DirectMap) {
        if region.kind == RegionKind::Ram {
            f(region.start, region.end());
        }
    }
}

/// Calls `f(start, end)` for memory that must never be handed out besides
/// the kernel image: the BIOS area and the boot information.
pub fn for_each_reserved(mut f: impl FnMut(u64, u64)) {
    f(0, ONE_MIB);
    for (start, end) in boot_info().occupied() {
        f(start, end);
    }
}

/// Maps device memory the kernel keeps using after the switch to its own
/// tables. x86_64 uses port I/O for its console and exit device: nothing.
pub fn map_devices(
    _pt: &PageTable<Format>,
    _alloc: &mut impl FnMut() -> Option<u64>,
) -> Result<(), MapError> {
    Ok(())
}
