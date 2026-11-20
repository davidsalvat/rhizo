//! What the platform tells the kernel at boot: the flattened device tree.

use super::MMIO_BASE;
use crate::memory::DirectMap;
use rhizo_core::fdt::{self, Fdt, Region};
use rhizo_mm::FRAME_SIZE;
use rhizo_mm::paging::{MapError, PageFlags, PageSize, PageTable};

/// Page-table entry format of this architecture.
pub type Format = rhizo_mm::paging::Aarch64;

/// The device tree passed at boot, read through the direct map.
pub fn device_tree() -> Fdt<'static> {
    let phys = super::boot::boot_arg();
    let base = DirectMap::virt(phys) as *const u8;
    // SAFETY: the boot protocol guarantees a device tree at `phys`, in RAM
    // below 4 GiB (mapped by the direct map); its header is 40 bytes.
    let head = unsafe { core::slice::from_raw_parts(base, 40) };
    let size = fdt::total_size(head).unwrap_or_else(|e| panic!("no device tree: {e:?}"));
    // SAFETY: the header declares `size` bytes of device tree at `base`, and
    // nothing ever writes to it.
    let blob = unsafe { core::slice::from_raw_parts(base, size) };
    Fdt::new(blob).unwrap_or_else(|e| panic!("invalid device tree: {e:?}"))
}

/// The kernel command line (`/chosen/bootargs`), copied into `buf`.
pub fn cmdline(buf: &mut [u8]) -> &[u8] {
    let args = device_tree().bootargs().ok().flatten().unwrap_or_default();
    let len = args.len().min(buf.len());
    buf[..len].copy_from_slice(&args[..len]);
    &buf[..len]
}

/// Calls `f(start, end)` for every RAM region.
pub fn for_each_ram(mut f: impl FnMut(u64, u64)) {
    device_tree()
        .memory_regions(|r| f(r.base, r.base + r.size))
        .unwrap_or_else(|e| panic!("cannot read memory from the device tree: {e:?}"));
}

/// Calls `f(start, end)` for memory that must never be handed out besides
/// the kernel image: the device tree itself.
pub fn for_each_reserved(mut f: impl FnMut(u64, u64)) {
    let phys = super::boot::boot_arg();
    let base = DirectMap::virt(phys) as *const u8;
    // SAFETY: as in `device_tree`.
    let head = unsafe { core::slice::from_raw_parts(base, 40) };
    let size = fdt::total_size(head).unwrap_or(0) as u64;
    f(phys, phys + size);
}

/// Which UART the device tree describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Uart {
    /// ARM PrimeCell PL011 (QEMU `virt`, Cloud Hypervisor).
    Pl011(Region),
    /// 16550-compatible, 8-bit registers (Firecracker).
    Ns16550(Region),
}

/// The boot UART from the device tree, if any.
pub fn uart() -> Option<Uart> {
    let dt = device_tree();
    if let Ok(Some(r)) = dt.find_compatible(b"arm,pl011") {
        return Some(Uart::Pl011(r));
    }
    if let Ok(Some(r)) = dt.find_compatible(b"ns16550a") {
        return Some(Uart::Ns16550(r));
    }
    None
}

/// GICv3 distributor and redistributors (one `RD_base` and `SGI_base`
/// pair per CPU): `(physical base, bytes)`.
pub fn gic_regions() -> Option<[(u64, u64); 2]> {
    let dt = device_tree();
    let d = dt.find_compatible_reg(b"arm,gic-v3", 0).ok()??;
    let r = dt.find_compatible_reg(b"arm,gic-v3", 1).ok()??;
    Some([(d.base, 0x1_0000), (r.base, r.size)])
}

/// Maps the devices the kernel keeps using as device memory in the MMIO
/// window, where the console already reaches its UART through the stage-1
/// tables: the UART page and the GIC (distributor, CPU 0 redistributor).
pub fn map_devices(
    pt: &PageTable<Format>,
    alloc: &mut impl FnMut() -> Option<u64>,
) -> Result<(), MapError> {
    let flags = PageFlags::WRITABLE | PageFlags::NO_EXECUTE | PageFlags::DEVICE;
    let mut map = |base: u64, len: u64| {
        let first = base & !(FRAME_SIZE - 1);
        for page in (first..base + len).step_by(FRAME_SIZE as usize) {
            pt.map(
                &mut DirectMap,
                alloc,
                MMIO_BASE + page,
                page,
                PageSize::Small,
                flags,
            )?;
        }
        Ok(())
    };
    if let Some(Uart::Pl011(r) | Uart::Ns16550(r)) = uart() {
        map(r.base, 1)?;
    }
    if let Some([(d, d_len), (r, _)]) = gic_regions() {
        map(d, d_len)?;
        map(r, 0x2_0000)?; // the boot CPU's; the others map theirs (`irq`)
    }
    #[cfg(any(feature = "ktest", feature = "bench-cell"))]
    {
        // Transports share pages: map each page once.
        let (mut result, mut last) = (Ok(()), u64::MAX);
        for_each_virtio_mmio(|base| {
            if result.is_ok() && base & !(FRAME_SIZE - 1) != last {
                last = base & !(FRAME_SIZE - 1);
                result = map(base, rhizo_virtio::REGION);
            }
        });
        result?;
    }
    Ok(())
}

/// Calls `f` with the physical base of every virtio-mmio transport the
/// device tree lists (QEMU `virt`, Firecracker).
#[cfg(any(feature = "ktest", feature = "bench-cell"))]
pub fn for_each_virtio_mmio(mut f: impl FnMut(u64)) {
    let _ = device_tree().for_each_compatible(b"virtio,mmio", |r| f(r.base));
}

/// The registers of a device at physical address `base`, mapped as device
/// memory in the MMIO window by [`map_devices`].
#[cfg(any(feature = "ktest", feature = "bench-cell"))]
#[derive(Debug)]
pub struct Mmio(u64);

#[cfg(any(feature = "ktest", feature = "bench-cell"))]
impl Mmio {
    pub fn new(base: u64) -> Self {
        Self(MMIO_BASE + base)
    }
}

#[cfg(any(feature = "ktest", feature = "bench-cell"))]
impl rhizo_hal::DeviceRegisters for Mmio {
    fn read32(&mut self, offset: usize) -> u32 {
        // SAFETY: `map_devices` mapped the device's registers as device
        // memory at this address; offsets stay within its region.
        unsafe { ((self.0 + offset as u64) as *const u32).read_volatile() }
    }

    fn write32(&mut self, offset: usize, value: u32) {
        // SAFETY: as in `read32`.
        unsafe { ((self.0 + offset as u64) as *mut u32).write_volatile(value) }
    }

    fn barrier(&mut self) {
        // SAFETY: a full-system data barrier only orders memory accesses.
        unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
    }
}
