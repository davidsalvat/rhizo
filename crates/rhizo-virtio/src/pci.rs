//! The virtio 1.x PCI transport, modern interface (virtio 1.2, section 4.1;
//! design D1 and D2 of `add-virtio-pci`).
//!
//! A function's configuration space is a [`DeviceRegisters`] over its 4 KiB
//! ECAM page. [`regions`] recognizes a virtio function of a given type
//! (network card, socket device), sizes its memory BARs, gives an address
//! to those the firmware left without one, turns on memory decoding and bus
//! mastering, and finds the structures its vendor capabilities point to.
//! The kernel maps them and hands them back as a [`Transport`] to
//! [`VirtioNet::probe_pci`] or [`VirtioVsock::probe_pci`].
//!
//! [`VirtioNet::probe_pci`]: crate::VirtioNet::probe_pci
//! [`VirtioVsock::probe_pci`]: crate::vsock::VirtioVsock::probe_pci

use rhizo_hal::DeviceRegisters;

/// Bytes of configuration space per function (ECAM).
pub const FUNCTION_BYTES: u64 = 4096;
/// Vendor ID of virtio devices.
pub const VIRTIO_VENDOR: u16 = 0x1af4;
/// Device IDs of modern virtio devices: this plus the device type.
const MODERN: u32 = 0x1040;
/// Device ID of a transitional network device (its subsystem ID is the
/// device type).
const NET_TRANSITIONAL: u16 = 0x1000;

/// Configuration-space offsets (PCI Local Bus 3.0, section 6.1).
const COMMAND: usize = 0x04;
const BAR0: usize = 0x10;
const SUBSYSTEM: usize = 0x2c;
const CAPABILITIES: usize = 0x34;
/// Command bits: memory space decoding, bus mastering (DMA).
const MEMORY_SPACE: u32 = 1 << 1;
const BUS_MASTER: u32 = 1 << 2;
/// Status bit (upper half of the command dword): a capability list exists.
const CAP_LIST: u32 = 1 << 20;
/// Capability ID of vendor-specific capabilities, which virtio uses.
const CAP_VENDOR: u8 = 0x09;
/// Most capabilities walked (a cycle in the list ends the walk).
const MAX_CAPS: usize = 48;

/// `cfg_type` of the virtio structures the driver uses.
const COMMON_CFG: u8 = 1;
const NOTIFY_CFG: u8 = 2;
const DEVICE_CFG: u8 = 4;

/// Offsets in the common configuration structure (virtio 1.2, 4.1.4.3).
pub mod common {
    pub const DEVICE_FEATURE_SELECT: usize = 0x00;
    pub const DEVICE_FEATURE: usize = 0x04;
    pub const DRIVER_FEATURE_SELECT: usize = 0x08;
    pub const DRIVER_FEATURE: usize = 0x0c;
    pub const DEVICE_STATUS: usize = 0x14;
    pub const QUEUE_SELECT: usize = 0x16;
    pub const QUEUE_SIZE: usize = 0x18;
    pub const QUEUE_ENABLE: usize = 0x1c;
    pub const QUEUE_NOTIFY_OFF: usize = 0x1e;
    pub const QUEUE_DESC: usize = 0x20;
    pub const QUEUE_DRIVER: usize = 0x28;
    pub const QUEUE_DEVICE: usize = 0x30;
}

/// Why a virtio network function could not be set up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PciError {
    /// A virtio structure (`cfg_type`) is missing, names no memory BAR, or
    /// runs past its BAR.
    Capability(u8),
    /// BAR `n` has no address and none could be given.
    Bar(u8),
}

/// A memory BAR: its address (0: none yet) and size in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bar {
    pub addr: u64,
    pub size: u64,
    /// Whether it takes two BAR slots (a 64-bit address).
    pub wide: bool,
}

/// Physical addresses free for BARs (from the device tree's `ranges`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub next: u64,
    pub end: u64,
}

impl Window {
    /// `size` bytes aligned on `size` (BARs are naturally aligned).
    fn take(&mut self, size: u64) -> Option<u64> {
        let at = self.next.checked_next_multiple_of(size)?;
        let end = at.checked_add(size)?;
        if end > self.end {
            return None;
        }
        self.next = end;
        Some(at)
    }
}

/// Where the registers of a virtio function are: each a physical address
/// and a length in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Regions {
    pub common: (u64, u64),
    pub notify: (u64, u64),
    /// Bytes between the notification addresses of consecutive
    /// `queue_notify_off` values.
    pub notify_multiplier: u32,
    pub device: (u64, u64),
}

/// The structures of a modern virtio-pci device, mapped by the kernel.
#[derive(Debug)]
pub struct Transport<R> {
    pub(crate) common: R,
    pub(crate) notify: R,
    pub(crate) notify_multiplier: u32,
    pub(crate) device: R,
    /// Each queue's `queue_notify_off`: its notification address is that
    /// times `notify_multiplier` into `notify`.
    pub(crate) notify_off: [u16; 3],
}

impl<R> Transport<R> {
    /// The registers at the addresses of [`Regions`]: common
    /// configuration, notifications and device configuration.
    pub fn new(common: R, notify: R, notify_multiplier: u32, device: R) -> Self {
        Self {
            common,
            notify,
            notify_multiplier,
            device,
            notify_off: [0; 3],
        }
    }
}

/// Whether the function behind `cfg` is a virtio device of type `device`
/// ([`NET_DEVICE`](crate::NET_DEVICE), [`VSOCK_DEVICE`](crate::VSOCK_DEVICE)).
pub fn is_virtio(cfg: &mut impl DeviceRegisters, device: u32) -> bool {
    let id = cfg.read32(0);
    if id as u16 != VIRTIO_VENDOR {
        return false;
    }
    match (id >> 16) as u16 {
        d if u32::from(d) == MODERN + device => true,
        NET_TRANSITIONAL => device == crate::NET_DEVICE && cfg.read32(SUBSYSTEM) >> 16 == 1,
        _ => false,
    }
}

/// What a BAR dword reads after all ones are written to it (its old value
/// is put back).
fn size_mask(cfg: &mut impl DeviceRegisters, at: usize) -> u32 {
    let old = cfg.read32(at);
    cfg.write32(at, !0);
    let mask = cfg.read32(at);
    cfg.write32(at, old);
    mask
}

/// The memory BARs of the function behind `cfg`, by slot (I/O BARs, unused
/// slots and the upper half of 64-bit BARs: `None`). Decoding is off while
/// they are sized.
pub fn memory_bars(cfg: &mut impl DeviceRegisters) -> [Option<Bar>; 6] {
    let command = cfg.read32(COMMAND) & 0xffff;
    cfg.write32(COMMAND, command & !MEMORY_SPACE & !1);
    let mut bars = [None; 6];
    let mut slot = 0;
    while slot < 6 {
        let at = BAR0 + slot * 4;
        let low = cfg.read32(at);
        let wide = low & 0b111 == 0b100;
        if low & 1 != 0 {
            slot += 1;
            continue;
        }
        let low_mask = size_mask(cfg, at) & !0xf;
        let (high, high_mask) = if wide && slot < 5 {
            (cfg.read32(at + 4), size_mask(cfg, at + 4))
        } else {
            (0, !0)
        };
        let mask = (u64::from(high_mask) << 32) | u64::from(low_mask);
        let addr = (u64::from(high) << 32) | u64::from(low & !0xf);
        let size = (!mask).wrapping_add(1);
        // A BAR decodes a naturally aligned power of two below the top of
        // the address space; a mask that says otherwise is a broken device
        // (finding S17 of `docs/security.md`), and its BAR is left out.
        if (low_mask != 0 || (wide && high_mask != 0))
            && size.is_power_of_two()
            && addr.checked_add(size).is_some()
        {
            bars[slot] = Some(Bar { addr, size, wide });
        }
        slot += if wide { 2 } else { 1 };
    }
    cfg.write32(COMMAND, command);
    bars
}

/// Gives every BAR in `bars` without an address one from `window` (none:
/// it stays unassigned) and writes it to the function.
pub fn assign_bars(
    cfg: &mut impl DeviceRegisters,
    bars: &mut [Option<Bar>; 6],
    mut window: Option<&mut Window>,
) {
    for (slot, bar) in bars.iter_mut().enumerate() {
        let Some(bar) = bar.as_mut().filter(|b| b.addr == 0) else {
            continue;
        };
        let Some(addr) = window.as_mut().and_then(|w| w.take(bar.size)) else {
            continue;
        };
        if !bar.wide && addr >> 32 != 0 {
            continue; // out of reach of a 32-bit BAR
        }
        let at = BAR0 + slot * 4;
        let kind = cfg.read32(at) & 0xf;
        cfg.write32(at, addr as u32 | kind);
        if bar.wide {
            cfg.write32(at + 4, (addr >> 32) as u32);
        }
        bar.addr = addr;
    }
}

/// A virtio structure: the BAR it is in, its offset and length there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Structure {
    bar: u8,
    offset: u32,
    length: u32,
}

/// The first structure of each `cfg_type` the driver uses: common,
/// notifications (and their multiplier), device.
fn structures(cfg: &mut impl DeviceRegisters) -> [Option<(Structure, u32)>; 3] {
    let mut found = [None; 3];
    if cfg.read32(COMMAND) & CAP_LIST == 0 {
        return found;
    }
    let mut at = cfg.read32(CAPABILITIES) as usize & 0xfc;
    for _ in 0..MAX_CAPS {
        if at == 0 {
            break;
        }
        let [id, next, _len, kind] = cfg.read32(at).to_le_bytes();
        let slot = match kind {
            COMMON_CFG => 0,
            NOTIFY_CFG => 1,
            DEVICE_CFG => 2,
            _ => 3,
        };
        if id == CAP_VENDOR && slot < 3 && found[slot].is_none() {
            let structure = Structure {
                bar: cfg.read32(at + 4) as u8,
                offset: cfg.read32(at + 8),
                length: cfg.read32(at + 12),
            };
            let multiplier = if kind == NOTIFY_CFG {
                cfg.read32(at + 16)
            } else {
                0
            };
            found[slot] = Some((structure, multiplier));
        }
        at = next as usize & 0xfc;
    }
    found
}

/// Sets up the virtio function of type `device` behind `cfg` for the
/// driver: its memory BARs get addresses (from `window` for those the
/// firmware left without one), memory decoding and bus mastering are turned
/// on, and the result says where its structures are. `Ok(None)`: not a
/// virtio device of that type.
pub fn regions(
    cfg: &mut impl DeviceRegisters,
    device: u32,
    window: Option<&mut Window>,
) -> Result<Option<Regions>, PciError> {
    if !is_virtio(cfg, device) {
        return Ok(None);
    }
    let mut bars = memory_bars(cfg);
    assign_bars(cfg, &mut bars, window);
    let found = structures(cfg);
    let mut regions = [(0, 0); 3];
    for (i, kind) in [COMMON_CFG, NOTIFY_CFG, DEVICE_CFG].into_iter().enumerate() {
        let (s, _) = found[i].ok_or(PciError::Capability(kind))?;
        let bar = bars
            .get(s.bar as usize)
            .copied()
            .flatten()
            .ok_or(PciError::Capability(kind))?;
        if bar.addr == 0 {
            return Err(PciError::Bar(s.bar));
        }
        if u64::from(s.offset) + u64::from(s.length) > bar.size {
            return Err(PciError::Capability(kind));
        }
        regions[i] = (bar.addr + u64::from(s.offset), u64::from(s.length));
    }
    let command = cfg.read32(COMMAND) & 0xffff;
    cfg.write32(COMMAND, command | MEMORY_SPACE | BUS_MASTER);
    Ok(Some(Regions {
        common: regions[0],
        notify: regions[1],
        notify_multiplier: found[1].map_or(0, |(_, m)| m),
        device: regions[2],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NET_DEVICE, VSOCK_DEVICE};

    const NET_MODERN: u16 = 0x1041;

    /// A function's configuration space: plain bytes, except BARs, which
    /// read back their size mask after all ones are written.
    struct Config {
        bytes: [u8; FUNCTION_BYTES as usize],
        /// Size of the BAR in each slot (0: none; a 64-bit BAR's size is
        /// in its first slot).
        sizes: [u64; 6],
        /// BAR slots written with all ones.
        probing: [bool; 6],
    }

    impl Config {
        fn new(vendor: u16, device: u16) -> Self {
            let mut c = Self {
                bytes: [0; FUNCTION_BYTES as usize],
                sizes: [0; 6],
                probing: [false; 6],
            };
            c.put32(0, u32::from(vendor) | (u32::from(device) << 16));
            c
        }

        fn put32(&mut self, at: usize, v: u32) {
            self.bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }

        fn get32(&self, at: usize) -> u32 {
            u32::from_le_bytes(self.bytes[at..at + 4].try_into().unwrap())
        }

        /// A BAR in `slot`: 32-bit or 64-bit memory, or I/O.
        fn bar(&mut self, slot: usize, size: u64, kind: u32, addr: u64) {
            self.sizes[slot] = size;
            self.put32(BAR0 + slot * 4, addr as u32 | kind);
            if kind & 0b110 == 0b100 {
                self.put32(BAR0 + slot * 4 + 4, (addr >> 32) as u32);
            }
        }

        /// The virtio vendor capabilities QEMU gives a network device:
        /// common, ISR, device and notify structures in BAR 4.
        fn virtio_caps(&mut self, bar: u8) {
            self.put32(COMMAND, CAP_LIST);
            self.bytes[CAPABILITIES] = 0x84;
            // An MSI-X capability first, then the virtio ones.
            self.put32(0x84, 0x11 | (0x40 << 8));
            let caps: [(usize, u8, u32, u32, usize); 4] = [
                (0x40, COMMON_CFG, 0, 0x1000, 0x50),
                (0x50, 3, 0x1000, 0x1000, 0x60),
                (0x60, DEVICE_CFG, 0x2000, 0x1000, 0x70),
                (0x70, NOTIFY_CFG, 0x3000, 0x1000, 0),
            ];
            for (at, kind, offset, length, next) in caps {
                self.put32(at, u32::from_le_bytes([CAP_VENDOR, next as u8, 16, kind]));
                self.put32(at + 4, u32::from(bar));
                self.put32(at + 8, offset);
                self.put32(at + 12, length);
            }
            self.put32(0x80, 4); // notify_off_multiplier
        }

        /// The size mask a BAR dword reads after all ones were written.
        fn mask(&self, at: usize) -> Option<u32> {
            let slot = at.checked_sub(BAR0)? / 4;
            if slot >= 6 || !at.is_multiple_of(4) || !self.probing[slot] {
                return None;
            }
            let low = self.get32(BAR0 + slot * 4);
            let (size, high_half) = match slot {
                s if self.sizes[s] != 0 => (self.sizes[s], false),
                s if s > 0 && self.get32(BAR0 + (s - 1) * 4) & 0b110 == 0b100 => {
                    (self.sizes[s - 1], true)
                }
                _ => return Some(0),
            };
            let mask = !(size.max(1) - 1);
            Some(if high_half {
                (mask >> 32) as u32
            } else {
                mask as u32 | (low & 0xf)
            })
        }
    }

    impl DeviceRegisters for Config {
        fn read32(&mut self, offset: usize) -> u32 {
            self.mask(offset).unwrap_or_else(|| self.get32(offset))
        }

        fn write32(&mut self, offset: usize, value: u32) {
            if offset == COMMAND {
                // The status half is read-only (or write-one-to-clear).
                let status = self.get32(COMMAND) & 0xffff_0000;
                return self.put32(COMMAND, (value & 0xffff) | status);
            }
            if let Some(slot) = offset.checked_sub(BAR0).map(|o| o / 4).filter(|&s| s < 6) {
                self.probing[slot] = value == !0;
                if value == !0 {
                    return;
                }
                if self.get32(offset) & 1 == 0 && self.sizes[slot] != 0 {
                    // Read-only low bits (type) are kept.
                    let kind = self.get32(offset) & 0xf;
                    return self.put32(offset, (value & !0xf) | kind);
                }
            }
            self.put32(offset, value);
        }

        fn read16(&mut self, _: usize) -> u16 {
            unreachable!("configuration space is read by dwords")
        }

        fn write16(&mut self, _: usize, _: u16) {
            unreachable!("configuration space is written by dwords")
        }

        fn read8(&mut self, _: usize) -> u8 {
            unreachable!("configuration space is read by dwords")
        }

        fn write8(&mut self, _: usize, _: u8) {
            unreachable!("configuration space is written by dwords")
        }

        fn barrier(&mut self) {}
    }

    /// A transitional virtio-net as QEMU builds it: I/O BAR 0, MSI-X in
    /// BAR 1, the virtio structures in 64-bit BAR 4.
    fn qemu_net() -> Config {
        let mut c = Config::new(VIRTIO_VENDOR, NET_TRANSITIONAL);
        c.put32(SUBSYSTEM, (1 << 16) | u32::from(VIRTIO_VENDOR));
        c.bar(0, 0x20, 1, 0);
        c.bar(1, 0x1000, 0, 0);
        c.bar(4, 0x4000, 0b1100, 0);
        c.virtio_caps(4);
        c
    }

    #[test]
    fn recognizes_modern_and_transitional_network_devices() {
        let is_net = |c: &mut Config| is_virtio(c, NET_DEVICE);
        assert!(is_net(&mut Config::new(VIRTIO_VENDOR, NET_MODERN)));
        assert!(is_net(&mut qemu_net()));
        let mut block = Config::new(VIRTIO_VENDOR, NET_TRANSITIONAL);
        block.put32(SUBSYSTEM, 2 << 16);
        assert!(!is_net(&mut block), "a transitional block device");
        assert!(!is_net(&mut Config::new(VIRTIO_VENDOR, 0x1042)));
        assert!(!is_net(&mut Config::new(0x8086, NET_MODERN)));
        assert!(!is_net(&mut Config::new(0xffff, 0xffff)), "no device");
    }

    #[test]
    fn recognizes_a_socket_device() {
        assert!(is_virtio(
            &mut Config::new(VIRTIO_VENDOR, 0x1053),
            VSOCK_DEVICE
        ));
        assert!(!is_virtio(
            &mut Config::new(VIRTIO_VENDOR, 0x1053),
            NET_DEVICE
        ));
        assert!(!is_virtio(&mut qemu_net(), VSOCK_DEVICE));
        let mut net = Config::new(VIRTIO_VENDOR, NET_TRANSITIONAL);
        net.put32(SUBSYSTEM, 1 << 16);
        assert!(
            !is_virtio(&mut net, VSOCK_DEVICE),
            "only network cards are transitional here"
        );
    }

    #[test]
    fn sizes_memory_bars_and_skips_io_bars() {
        let mut c = qemu_net();
        c.bar(2, 0x10_0000, 0, 0xfe00_0000);
        let bars = memory_bars(&mut c);
        assert_eq!(bars[0], None, "I/O");
        assert_eq!(
            bars[1],
            Some(Bar {
                addr: 0,
                size: 0x1000,
                wide: false
            })
        );
        assert_eq!(
            bars[2].map(|b| (b.addr, b.size)),
            Some((0xfe00_0000, 0x10_0000))
        );
        assert_eq!(bars[3], None);
        assert_eq!(
            bars[4],
            Some(Bar {
                addr: 0,
                size: 0x4000,
                wide: true
            })
        );
        assert_eq!(bars[5], None, "upper half of BAR 4");
        assert_eq!(c.get32(BAR0 + 8), 0xfe00_0000, "addresses restored");
    }

    #[test]
    fn a_64_bit_bar_keeps_its_high_address() {
        let mut c = Config::new(VIRTIO_VENDOR, NET_MODERN);
        c.bar(0, 0x8_0000, 0b1100, 0x3fff_fff8_0000);
        assert_eq!(
            memory_bars(&mut c)[0],
            Some(Bar {
                addr: 0x3fff_fff8_0000,
                size: 0x8_0000,
                wide: true
            })
        );
    }

    #[test]
    fn unassigned_bars_get_aligned_addresses_from_the_window() {
        let mut c = qemu_net();
        let mut window = Window {
            next: 0x1000_0800,
            end: 0x1001_0000,
        };
        let r = regions(&mut c, NET_DEVICE, Some(&mut window))
            .unwrap()
            .unwrap();
        assert_eq!(c.get32(BAR0 + 4), 0x1000_1000, "BAR 1, aligned");
        assert_eq!(c.get32(BAR0 + 16), 0x1000_4000 | 0b1100, "BAR 4, aligned");
        assert_eq!(c.get32(BAR0 + 20), 0);
        assert_eq!(c.get32(BAR0), 1, "the I/O BAR is left alone");
        assert_eq!(window.next, 0x1000_8000);
        assert_eq!(
            r,
            Regions {
                common: (0x1000_4000, 0x1000),
                notify: (0x1000_7000, 0x1000),
                notify_multiplier: 4,
                device: (0x1000_6000, 0x1000),
            }
        );
        assert_eq!(
            c.get32(COMMAND) & 0xffff,
            MEMORY_SPACE | BUS_MASTER,
            "decoding and DMA on"
        );
    }

    #[test]
    fn firmware_addresses_are_kept() {
        let mut c = qemu_net();
        c.bar(4, 0x4000, 0b1100, 0x80_0000_0000);
        let r = regions(&mut c, NET_DEVICE, None).unwrap().unwrap();
        assert_eq!(r.common, (0x80_0000_0000, 0x1000));
    }

    #[test]
    fn refuses_what_it_cannot_reach() {
        assert_eq!(
            regions(&mut Config::new(0x8086, 0x10d3), NET_DEVICE, None),
            Ok(None),
            "not virtio"
        );
        assert_eq!(
            regions(&mut qemu_net(), NET_DEVICE, None),
            Err(PciError::Bar(4)),
            "no address and no window"
        );
        let mut small = Window {
            next: 0x1000_0000,
            end: 0x1000_2000,
        };
        assert_eq!(
            regions(&mut qemu_net(), NET_DEVICE, Some(&mut small)),
            Err(PciError::Bar(4)),
            "window too small"
        );
        let mut c = Config::new(VIRTIO_VENDOR, NET_MODERN);
        c.bar(0, 0x1000, 0, 0x1000_0000);
        assert_eq!(
            regions(&mut c, NET_DEVICE, None),
            Err(PciError::Capability(COMMON_CFG)),
            "no capabilities"
        );
        c.virtio_caps(0);
        assert_eq!(
            regions(&mut c, NET_DEVICE, None),
            Err(PciError::Capability(NOTIFY_CFG)),
            "structures past the end of the BAR"
        );
        c.virtio_caps(5);
        assert_eq!(
            regions(&mut c, NET_DEVICE, None),
            Err(PciError::Capability(COMMON_CFG)),
            "a BAR that does not exist"
        );
    }

    #[test]
    fn a_capability_cycle_ends_the_walk() {
        let mut c = qemu_net();
        c.bytes[0x71] = 0x40; // notify points back to common
        c.bar(4, 0x4000, 0b1100, 0x8000_0000);
        assert!(regions(&mut c, NET_DEVICE, None).unwrap().is_some());
        c.bytes[0x51] = 0x40; // ISR loops before device and notify
        assert_eq!(
            regions(&mut c, NET_DEVICE, None),
            Err(PciError::Capability(NOTIFY_CFG))
        );
    }

    /// Configuration space from a broken device (spec `networking`,
    /// virtio-pci): a QEMU network function with any dwords changed and any
    /// BAR sizes. Setting it up never panics, and what it finds lies inside
    /// one of its BARs, without wrapping around.
    mod hostile {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn any_configuration_space_is_set_up_or_refused(
                edits in prop::collection::vec((0usize..64, any::<u32>()), 0..6),
                sizes in [0u32..64, 0u32..64, 0u32..64, 0u32..64, 0u32..64, 0u32..64],
                window in prop_oneof![
                    Just(None),
                    Just(Some((0x1000_0000, 0x2000_0000))),
                    prop::option::of((any::<u64>(), any::<u64>())),
                ],
            ) {
                let mut c = qemu_net();
                for (dword, value) in edits {
                    c.put32(dword * 4, value);
                }
                for (slot, &bits) in sizes.iter().enumerate() {
                    if bits < 40 {
                        c.sizes[slot] = if bits < 4 { 0 } else { 1u64 << bits };
                    }
                }
                let mut w = window.map(|(next, end)| Window { next, end });
                if let Ok(Some(r)) = regions(&mut c, NET_DEVICE, w.as_mut()) {
                    let bars = memory_bars(&mut c);
                    for (addr, len) in [r.common, r.notify, r.device] {
                        let inside = bars.iter().flatten().any(|b| {
                            addr >= b.addr
                                && addr.checked_add(len).is_some_and(|e| Some(e) <= b.addr.checked_add(b.size))
                        });
                        prop_assert!(inside, "{addr:#x} + {len:#x} in {bars:x?}");
                    }
                }
            }
        }
    }
}
