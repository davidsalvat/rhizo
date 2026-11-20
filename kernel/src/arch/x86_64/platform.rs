//! What the platform tells the kernel at boot (PVH `hvm_start_info`).

use crate::memory::DirectMap;
use rhizo_mm::memmap::{BootInfo, RegionKind};
use rhizo_mm::paging::{MapError, PageTable};
#[cfg(any(feature = "ktest", feature = "bench-cell"))]
use rhizo_mm::paging::{PageFlags, PageSize};

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
/// tables. x86_64 uses port I/O for its console and exit device; builds
/// that run cells map the virtio-mmio transports, uncached, where the
/// direct map would have them.
#[cfg_attr(
    not(any(feature = "ktest", feature = "bench-cell")),
    allow(unused_variables, reason = "nothing to map without cells")
)]
pub fn map_devices(
    pt: &PageTable<Format>,
    alloc: &mut impl FnMut() -> Option<u64>,
) -> Result<(), MapError> {
    #[cfg(any(feature = "ktest", feature = "bench-cell"))]
    {
        let flags = PageFlags::WRITABLE | PageFlags::NO_EXECUTE | PageFlags::DEVICE;
        // Transports share pages: map each page once.
        let (mut result, mut last) = (Ok(()), u64::MAX);
        for_each_virtio_mmio(|base| {
            let page = base & !0xfff;
            if result.is_ok() && page != last {
                last = page;
                result = pt.map(
                    &mut DirectMap,
                    alloc,
                    DirectMap::virt(page),
                    page,
                    PageSize::Small,
                    flags,
                );
            }
        });
        result?;
    }
    Ok(())
}

/// Calls `f` with the physical base of every virtio-mmio transport the
/// command line lists (`virtio_mmio.device=`, from QEMU `microvm` and
/// Firecracker).
#[cfg(any(feature = "ktest", feature = "bench-cell"))]
pub fn for_each_virtio_mmio(mut f: impl FnMut(u64)) {
    let mut buf = [0u8; 1024];
    rhizo_core::options::virtio_mmio_devices(cmdline(&mut buf), |base, _| f(base));
}

/// The registers of a device at physical address `base`, mapped uncached
/// by [`map_devices`].
#[cfg(any(feature = "ktest", feature = "bench-cell"))]
#[derive(Debug)]
pub struct Mmio(u64);

#[cfg(any(feature = "ktest", feature = "bench-cell"))]
impl Mmio {
    pub fn new(base: u64) -> Self {
        Self(DirectMap::virt(base))
    }
}

#[cfg(any(feature = "ktest", feature = "bench-cell"))]
impl rhizo_hal::DeviceRegisters for Mmio {
    fn read32(&mut self, offset: usize) -> u32 {
        // SAFETY: `map_devices` mapped the device's registers uncached at
        // this address; offsets stay within its region.
        unsafe { ((self.0 + offset as u64) as *const u32).read_volatile() }
    }

    fn write32(&mut self, offset: usize, value: u32) {
        // SAFETY: as in `read32`.
        unsafe { ((self.0 + offset as u64) as *mut u32).write_volatile(value) }
    }

    fn barrier(&mut self) {
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    }
}
