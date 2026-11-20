//! A virtio network device (spec `networking`, design D1 and D3 of
//! `add-virtio-net`, D1 of `add-virtio-pci`).
//!
//! [`VirtioNet::probe`] brings up a virtio-mmio device (version 2, the
//! modern interface) and [`VirtioNet::probe_pci`] a modern virtio-pci one
//! (found and set up with [`pci`]), each with a receive and a transmit
//! split virtqueue, and the result is a [`NetLink`]. Registers are reached
//! through [`DeviceRegisters`] and the rings and buffers through
//! [`PhysicalMemory`]: the driver runs on the host against a simulated
//! device. It polls: the device is asked for no interrupts.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use rhizo_hal::{DeviceRegisters, NetLink, PhysicalMemory};

pub mod pci;

/// virtio-mmio register offsets (virtio 1.2, section 4.2.2).
pub mod reg {
    pub const MAGIC: usize = 0x000;
    pub const VERSION: usize = 0x004;
    pub const DEVICE_ID: usize = 0x008;
    pub const DEVICE_FEATURES: usize = 0x010;
    pub const DEVICE_FEATURES_SEL: usize = 0x014;
    pub const DRIVER_FEATURES: usize = 0x020;
    pub const DRIVER_FEATURES_SEL: usize = 0x024;
    pub const QUEUE_SEL: usize = 0x030;
    pub const QUEUE_NUM_MAX: usize = 0x034;
    pub const QUEUE_NUM: usize = 0x038;
    pub const QUEUE_READY: usize = 0x044;
    pub const QUEUE_NOTIFY: usize = 0x050;
    pub const STATUS: usize = 0x070;
    pub const QUEUE_DESC_LOW: usize = 0x080;
    pub const QUEUE_DESC_HIGH: usize = 0x084;
    pub const QUEUE_DRIVER_LOW: usize = 0x090;
    pub const QUEUE_DRIVER_HIGH: usize = 0x094;
    pub const QUEUE_DEVICE_LOW: usize = 0x0a0;
    pub const QUEUE_DEVICE_HIGH: usize = 0x0a4;
    pub const CONFIG: usize = 0x100;
}

/// `"virt"`, little endian.
pub const MAGIC: u32 = 0x7472_6976;
/// Device ID of a network card.
pub const NET_DEVICE: u32 = 1;
/// Bytes of registers a virtio-mmio device takes.
pub const REGION: u64 = 0x200;

const ACKNOWLEDGE: u32 = 1;
const DRIVER: u32 = 2;
const DRIVER_OK: u32 = 4;
const FEATURES_OK: u32 = 8;
const F_MAC: u32 = 1 << 5;
/// `VIRTIO_F_VERSION_1`, bit 32: the second feature word's bit 0.
const F_VERSION_1: u32 = 1;
const DESC_F_WRITE: u16 = 2;
const AVAIL_F_NO_INTERRUPT: u16 = 1;
const USED_F_NO_NOTIFY: u16 = 1;

/// Entries per queue. The receive queue offers `QUEUE_SIZE * BUF` bytes,
/// which stay above [`RX_MIN_OFFERED`] while up to 31 frames wait to be
/// taken.
pub const QUEUE_SIZE: u16 = 64;
/// Bytes per buffer: a header and a full Ethernet frame fit.
const BUF: u64 = 2048;
/// `struct virtio_net_hdr` with `VIRTIO_F_VERSION_1`.
const HDR: usize = 12;
/// Receive buffer bytes a device may wait for before it hands over a frame:
/// the largest frame it takes from the host and its header. Firecracker
/// (`MAX_BUFFER_SIZE`) reads from its tap only while the receive buffers
/// offered add up to this much, whatever features were negotiated.
const RX_MIN_OFFERED: u64 = 65_562;
const _: () = assert!((QUEUE_SIZE as u64 - 1) * BUF >= RX_MIN_OFFERED);
/// Largest frame sent or received (without the header).
pub const FRAME_MAX: usize = BUF as usize - HDR;
const FRAME: u64 = 4096;
/// Frames [`VirtioNet::probe`] takes: per queue, its rings and its buffers.
pub const FRAMES: usize = 2 * (1 + (QUEUE_SIZE as u64 * BUF / FRAME) as usize);
/// Layout of a queue in its frame: descriptors, then the available ring,
/// then the used ring.
const AVAIL_AT: u64 = 1024;
const USED_AT: u64 = 2048;
const _: () = assert!(QUEUE_SIZE as u64 * 16 <= AVAIL_AT);
const _: () = assert!(AVAIL_AT + 6 + QUEUE_SIZE as u64 * 2 <= USED_AT);
const _: () = assert!(USED_AT + 6 + QUEUE_SIZE as u64 * 8 <= FRAME);

/// Why a device was not brought up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeError {
    /// No virtio-mmio registers here.
    NotVirtio,
    /// A legacy (or unknown) virtio-mmio version.
    Version(u32),
    /// Not a network card (0: no device behind the transport).
    NotNet(u32),
    /// The device does not offer `VIRTIO_F_VERSION_1`, or refused ours.
    Features,
    /// A queue is missing or already in use.
    Queue(u16),
    /// No frame for the rings or buffers.
    OutOfMemory,
}

/// How the driver reaches the device.
#[derive(Debug)]
enum Transport<R> {
    Mmio(R),
    Pci(pci::Transport<R>),
}

impl<R: DeviceRegisters> Transport<R> {
    fn set_status(&mut self, status: u32) {
        match self {
            Self::Mmio(r) => r.write32(reg::STATUS, status),
            Self::Pci(p) => p.common.write8(pci::common::DEVICE_STATUS, status as u8),
        }
    }

    fn status(&mut self) -> u32 {
        match self {
            Self::Mmio(r) => r.read32(reg::STATUS),
            Self::Pci(p) => p.common.read8(pci::common::DEVICE_STATUS).into(),
        }
    }

    /// Feature bits `32 * word ..` the device offers.
    fn device_features(&mut self, word: u32) -> u32 {
        let (r, select, features) = match self {
            Self::Mmio(r) => (r, reg::DEVICE_FEATURES_SEL, reg::DEVICE_FEATURES),
            Self::Pci(p) => (
                &mut p.common,
                pci::common::DEVICE_FEATURE_SELECT,
                pci::common::DEVICE_FEATURE,
            ),
        };
        r.write32(select, word);
        r.read32(features)
    }

    fn set_driver_features(&mut self, word: u32, bits: u32) {
        let (r, select, features) = match self {
            Self::Mmio(r) => (r, reg::DRIVER_FEATURES_SEL, reg::DRIVER_FEATURES),
            Self::Pci(p) => (
                &mut p.common,
                pci::common::DRIVER_FEATURE_SELECT,
                pci::common::DRIVER_FEATURE,
            ),
        };
        r.write32(select, word);
        r.write32(features, bits);
    }

    /// The MAC address in the device configuration.
    fn mac(&mut self) -> [u8; 6] {
        match self {
            Self::Mmio(r) => {
                let (a, b) = (r.read32(reg::CONFIG), r.read32(reg::CONFIG + 4));
                let (a, b) = (a.to_le_bytes(), b.to_le_bytes());
                [a[0], a[1], a[2], a[3], b[0], b[1]]
            }
            Self::Pci(p) => core::array::from_fn(|i| p.device.read8(i)),
        }
    }

    /// Selects queue `index`: its largest size, `None` when it is in use.
    fn queue_max(&mut self, index: u16) -> Option<u32> {
        let (max, enabled) = match self {
            Self::Mmio(r) => {
                r.write32(reg::QUEUE_SEL, u32::from(index));
                (r.read32(reg::QUEUE_NUM_MAX), r.read32(reg::QUEUE_READY))
            }
            Self::Pci(p) => {
                p.common.write16(pci::common::QUEUE_SELECT, index);
                let max = p.common.read16(pci::common::QUEUE_SIZE);
                (
                    max.into(),
                    p.common.read16(pci::common::QUEUE_ENABLE).into(),
                )
            }
        };
        (enabled == 0).then_some(max)
    }

    /// Gives the selected queue `index` its size and the addresses of its
    /// descriptors, available ring and used ring, and enables it.
    fn enable_queue(&mut self, index: u16, size: u16, rings: [u64; 3]) {
        match self {
            Self::Mmio(r) => {
                r.write32(reg::QUEUE_NUM, u32::from(size));
                let lows = [
                    reg::QUEUE_DESC_LOW,
                    reg::QUEUE_DRIVER_LOW,
                    reg::QUEUE_DEVICE_LOW,
                ];
                for (low, at) in lows.into_iter().zip(rings) {
                    r.write32(low, at as u32);
                    r.write32(low + 4, (at >> 32) as u32);
                }
                r.write32(reg::QUEUE_READY, 1);
            }
            Self::Pci(p) => {
                let c = &mut p.common;
                c.write16(pci::common::QUEUE_SIZE, size);
                let lows = [
                    pci::common::QUEUE_DESC,
                    pci::common::QUEUE_DRIVER,
                    pci::common::QUEUE_DEVICE,
                ];
                for (low, at) in lows.into_iter().zip(rings) {
                    c.write32(low, at as u32);
                    c.write32(low + 4, (at >> 32) as u32);
                }
                let off = c.read16(pci::common::QUEUE_NOTIFY_OFF);
                p.notify_at[index as usize] = usize::from(off) * p.notify_multiplier as usize;
                c.write16(pci::common::QUEUE_ENABLE, 1);
            }
        }
    }

    /// Tells the device queue `index` has new buffers.
    fn notify(&mut self, index: u16) {
        match self {
            Self::Mmio(r) => r.write32(reg::QUEUE_NOTIFY, u32::from(index)),
            Self::Pci(p) => p.notify.write16(p.notify_at[index as usize], index),
        }
    }

    fn barrier(&mut self) {
        match self {
            Self::Mmio(r) => r.barrier(),
            Self::Pci(p) => p.common.barrier(),
        }
    }
}

/// One split virtqueue, in one frame.
#[derive(Debug)]
struct Queue {
    index: u16,
    frame: u64,
    /// Next index of the available ring (ours).
    avail: u16,
    /// Next index of the used ring to read.
    used: u16,
    /// Buffer of each descriptor.
    bufs: Vec<u64>,
}

impl Queue {
    fn read16(mem: &impl PhysicalMemory, at: u64) -> u16 {
        let mut b = [0u8; 2];
        mem.read_bytes(at, &mut b);
        u16::from_le_bytes(b)
    }

    /// Sets descriptor `id` to its buffer, `len` bytes, `flags`.
    fn describe(&self, mem: &mut impl PhysicalMemory, id: u16, len: u32, flags: u16) {
        let mut d = [0u8; 16];
        d[..8].copy_from_slice(&self.bufs[id as usize].to_le_bytes());
        d[8..12].copy_from_slice(&len.to_le_bytes());
        d[12..14].copy_from_slice(&flags.to_le_bytes());
        mem.write_bytes(self.frame + u64::from(id) * 16, &d);
    }

    /// Offers descriptor `id` to the device (the index is published by
    /// [`publish`](Self::publish)).
    fn offer(&mut self, mem: &mut impl PhysicalMemory, id: u16) {
        let slot = self.frame + AVAIL_AT + 4 + u64::from(self.avail % QUEUE_SIZE) * 2;
        mem.write_bytes(slot, &id.to_le_bytes());
        self.avail = self.avail.wrapping_add(1);
    }

    /// Publishes the offered descriptors and tells the device, unless it
    /// asked not to be told.
    fn publish<R: DeviceRegisters>(&self, mem: &mut impl PhysicalMemory, t: &mut Transport<R>) {
        t.barrier();
        mem.write_bytes(self.frame + AVAIL_AT + 2, &self.avail.to_le_bytes());
        t.barrier();
        let flags = Self::read16(mem, self.frame + USED_AT);
        if flags & USED_F_NO_NOTIFY == 0 {
            t.notify(self.index);
        }
    }

    /// The next descriptor the device used, and how many bytes it wrote.
    fn take_used(&mut self, mem: &impl PhysicalMemory) -> Option<(u16, u32)> {
        if Self::read16(mem, self.frame + USED_AT + 2) == self.used {
            return None;
        }
        let mut e = [0u8; 8];
        let slot = self.frame + USED_AT + 4 + u64::from(self.used % QUEUE_SIZE) * 8;
        mem.read_bytes(slot, &mut e);
        self.used = self.used.wrapping_add(1);
        let id = u32::from_le_bytes([e[0], e[1], e[2], e[3]]);
        let len = u32::from_le_bytes([e[4], e[5], e[6], e[7]]);
        Some(((id % u32::from(QUEUE_SIZE)) as u16, len))
    }
}

/// A virtio network card.
#[derive(Debug)]
pub struct VirtioNet<R, M> {
    regs: Transport<R>,
    mem: M,
    mac: [u8; 6],
    rx: Queue,
    tx: Queue,
    /// Transmit descriptors the device gave back.
    tx_free: Vec<u16>,
    frame: Vec<u8>,
}

impl<R: DeviceRegisters, M: PhysicalMemory> VirtioNet<R, M> {
    /// Brings up the network card behind `regs`, taking frames for its
    /// rings and buffers from `alloc` (never returned: the card lives as
    /// long as the kernel).
    pub fn probe(
        mut regs: R,
        mem: M,
        alloc: &mut dyn FnMut() -> Option<u64>,
    ) -> Result<Self, ProbeError> {
        if regs.read32(reg::MAGIC) != MAGIC {
            return Err(ProbeError::NotVirtio);
        }
        // An empty transport (device 0) is not an error, whatever its
        // version.
        match regs.read32(reg::DEVICE_ID) {
            NET_DEVICE => {}
            id => return Err(ProbeError::NotNet(id)),
        }
        match regs.read32(reg::VERSION) {
            2 => {}
            v => return Err(ProbeError::Version(v)),
        }
        Self::start(Transport::Mmio(regs), mem, alloc)
    }

    /// Brings up the modern virtio-pci network card whose structures
    /// `transport` reaches (see [`pci::net_regions`]), taking frames as
    /// [`probe`](Self::probe) does.
    pub fn probe_pci(
        transport: pci::Transport<R>,
        mem: M,
        alloc: &mut dyn FnMut() -> Option<u64>,
    ) -> Result<Self, ProbeError> {
        Self::start(Transport::Pci(transport), mem, alloc)
    }

    /// Driver initialization (virtio 1.2, section 3.1.1).
    fn start(
        mut regs: Transport<R>,
        mut mem: M,
        alloc: &mut dyn FnMut() -> Option<u64>,
    ) -> Result<Self, ProbeError> {
        regs.set_status(0);
        while regs.status() != 0 {
            core::hint::spin_loop(); // the reset is not done yet
        }
        regs.set_status(ACKNOWLEDGE);
        regs.set_status(ACKNOWLEDGE | DRIVER);
        let low = regs.device_features(0);
        let high = regs.device_features(1);
        if high & F_VERSION_1 == 0 {
            return Err(ProbeError::Features);
        }
        regs.set_driver_features(0, low & F_MAC);
        regs.set_driver_features(1, F_VERSION_1);
        regs.set_status(ACKNOWLEDGE | DRIVER | FEATURES_OK);
        if regs.status() & FEATURES_OK == 0 {
            return Err(ProbeError::Features);
        }
        let mac = if low & F_MAC != 0 {
            regs.mac()
        } else {
            [0x02, 0, 0, 0, 0, 1] // locally administered
        };
        let rx = Self::queue(&mut regs, &mut mem, alloc, 0)?;
        let tx = Self::queue(&mut regs, &mut mem, alloc, 1)?;
        let mut card = Self {
            regs,
            mem,
            mac,
            rx,
            tx,
            tx_free: (0..QUEUE_SIZE).rev().collect(),
            frame: vec![0; FRAME_MAX],
        };
        for id in 0..QUEUE_SIZE {
            card.rx
                .describe(&mut card.mem, id, BUF as u32, DESC_F_WRITE);
            card.rx.offer(&mut card.mem, id);
        }
        card.regs
            .set_status(ACKNOWLEDGE | DRIVER | FEATURES_OK | DRIVER_OK);
        card.rx.publish(&mut card.mem, &mut card.regs);
        Ok(card)
    }

    /// Sets up queue `index` in a new frame, with buffers in more frames.
    fn queue(
        regs: &mut Transport<R>,
        mem: &mut M,
        alloc: &mut dyn FnMut() -> Option<u64>,
        index: u16,
    ) -> Result<Queue, ProbeError> {
        if regs
            .queue_max(index)
            .is_none_or(|max| max < u32::from(QUEUE_SIZE))
        {
            return Err(ProbeError::Queue(index));
        }
        let mut frame = || {
            let f = alloc().ok_or(ProbeError::OutOfMemory)?;
            mem.zero_frame(f);
            Ok::<u64, ProbeError>(f)
        };
        let ring = frame()?;
        let mut bufs = Vec::with_capacity(QUEUE_SIZE as usize);
        while bufs.len() < QUEUE_SIZE as usize {
            let f = frame()?;
            bufs.extend((0..FRAME / BUF).map(|i| f + i * BUF));
        }
        // The driver polls: no interrupts, please.
        mem.write_bytes(ring + AVAIL_AT, &AVAIL_F_NO_INTERRUPT.to_le_bytes());
        regs.enable_queue(index, QUEUE_SIZE, [ring, ring + AVAIL_AT, ring + USED_AT]);
        Ok(Queue {
            index,
            frame: ring,
            avail: 0,
            used: 0,
            bufs,
        })
    }
}

impl<R: DeviceRegisters, M: PhysicalMemory> NetLink for VirtioNet<R, M> {
    fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn send(&mut self, frame: &[u8]) -> bool {
        self.regs.barrier();
        while let Some((id, _)) = self.tx.take_used(&self.mem) {
            self.tx_free.push(id);
        }
        if frame.len() > FRAME_MAX {
            return false;
        }
        let Some(id) = self.tx_free.pop() else {
            return false;
        };
        let buf = self.tx.bufs[id as usize];
        self.mem.write_bytes(buf, &[0; HDR]);
        self.mem.write_bytes(buf + HDR as u64, frame);
        self.tx
            .describe(&mut self.mem, id, (HDR + frame.len()) as u32, 0);
        self.tx.offer(&mut self.mem, id);
        self.tx.publish(&mut self.mem, &mut self.regs);
        true
    }

    fn recv(&mut self, take: &mut dyn FnMut(&[u8])) -> bool {
        self.regs.barrier();
        let Some((id, len)) = self.rx.take_used(&self.mem) else {
            return false;
        };
        let n = (len as usize).saturating_sub(HDR).min(FRAME_MAX);
        let buf = self.rx.bufs[id as usize];
        self.mem.read_bytes(buf + HDR as u64, &mut self.frame[..n]);
        // The buffer goes back to the device before the frame is handed on.
        self.rx.offer(&mut self.mem, id);
        self.rx.publish(&mut self.mem, &mut self.regs);
        take(&self.frame[..n]);
        true
    }
}

#[cfg(test)]
mod tests;
