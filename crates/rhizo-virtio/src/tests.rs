//! Host tests against a simulated virtio-mmio network device that reads and
//! writes the rings in fake physical memory, as a VMM would.

use super::*;
use rhizo_hal::testing::FakePhysicalMemory;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Debug, Default, Clone, Copy)]
struct QueueState {
    num: u32,
    ready: u32,
    desc: u64,
    avail: u64,
    used: u64,
    /// Next available entry the device takes.
    next: u16,
}

#[derive(Debug)]
struct Device {
    mem: FakePhysicalMemory,
    version: u32,
    id: u32,
    features: [u32; 2],
    driver: [u32; 2],
    features_sel: usize,
    driver_sel: usize,
    status: u32,
    queue_sel: usize,
    queues: [QueueState; 2],
    sent: Vec<Vec<u8>>,
    notifies: [usize; 2],
}

impl Device {
    fn new() -> Self {
        Self {
            mem: FakePhysicalMemory::new(),
            version: 2,
            id: NET_DEVICE,
            features: [F_MAC, F_VERSION_1],
            driver: [0; 2],
            features_sel: 0,
            driver_sel: 0,
            status: 0,
            queue_sel: 0,
            queues: [QueueState::default(); 2],
            sent: Vec::new(),
            notifies: [0; 2],
        }
    }

    fn read16(&self, at: u64) -> u16 {
        let mut b = [0u8; 2];
        self.mem.read_bytes(at, &mut b);
        u16::from_le_bytes(b)
    }

    /// The next buffer the driver offered on queue `q`: (id, address, length).
    fn next_buffer(&mut self, q: usize) -> Option<(u16, u64, u32)> {
        let s = self.queues[q];
        if self.read16(s.avail + 2) == s.next {
            return None;
        }
        let id = self.read16(s.avail + 4 + u64::from(s.next % s.num as u16) * 2);
        let mut d = [0u8; 16];
        self.mem.read_bytes(s.desc + u64::from(id) * 16, &mut d);
        self.queues[q].next = s.next.wrapping_add(1);
        let addr = u64::from_le_bytes(d[..8].try_into().unwrap());
        Some((id, addr, u32::from_le_bytes(d[8..12].try_into().unwrap())))
    }

    fn give_back(&mut self, q: usize, id: u16, len: u32) {
        let s = self.queues[q];
        let idx = self.read16(s.used + 2);
        let slot = s.used + 4 + u64::from(idx % s.num as u16) * 8;
        let mut e = [0u8; 8];
        e[..4].copy_from_slice(&u32::from(id).to_le_bytes());
        e[4..].copy_from_slice(&len.to_le_bytes());
        self.mem.write_bytes(slot, &e);
        self.mem
            .write_bytes(s.used + 2, &idx.wrapping_add(1).to_le_bytes());
    }

    /// Queue `q` was notified: the transmit queue sends everything offered.
    fn notify(&mut self, q: usize) {
        self.notifies[q] += 1;
        while q == 1
            && let Some((id, addr, len)) = self.next_buffer(1)
        {
            let mut frame = vec![0u8; len as usize - HDR];
            self.mem.read_bytes(addr + HDR as u64, &mut frame);
            self.sent.push(frame);
            self.give_back(1, id, 0);
        }
    }

    /// Takes a status write: FEATURES_OK only sticks if the driver took
    /// VERSION_1.
    fn set_status(&mut self, value: u32) {
        let refused = value & FEATURES_OK != 0 && self.driver[1] & F_VERSION_1 == 0;
        self.status = if refused { value & !FEATURES_OK } else { value };
    }

    /// Bytes of the receive buffers offered and not yet taken.
    fn rx_offered(&self) -> u64 {
        let s = self.queues[0];
        let mut next = s.next;
        let mut bytes = 0;
        while next != self.read16(s.avail + 2) {
            let id = self.read16(s.avail + 4 + u64::from(next % s.num as u16) * 2);
            let mut len = [0u8; 4];
            self.mem
                .read_bytes(s.desc + u64::from(id) * 16 + 8, &mut len);
            bytes += u64::from(u32::from_le_bytes(len));
            next = next.wrapping_add(1);
        }
        bytes
    }

    /// Delivers `frame` as Firecracker does: only while the receive buffers
    /// offered add up to its largest frame and header.
    fn deliver_like_firecracker(&mut self, frame: &[u8]) -> bool {
        self.rx_offered() >= RX_MIN_OFFERED && self.deliver(frame)
    }

    /// Puts `frame` in the next receive buffer; `false` if none is offered.
    fn deliver(&mut self, frame: &[u8]) -> bool {
        let Some((id, addr, len)) = self.next_buffer(0) else {
            return false;
        };
        assert!(frame.len() + HDR <= len as usize);
        self.mem.write_bytes(addr, &[0; HDR]);
        self.mem.write_bytes(addr + HDR as u64, frame);
        self.give_back(0, id, (HDR + frame.len()) as u32);
        true
    }
}

#[derive(Debug, Clone)]
struct Regs(Rc<RefCell<Device>>);

impl DeviceRegisters for Regs {
    fn read32(&mut self, offset: usize) -> u32 {
        let d = self.0.borrow();
        match offset {
            reg::MAGIC => MAGIC,
            reg::VERSION => d.version,
            reg::DEVICE_ID => d.id,
            reg::DEVICE_FEATURES => d.features[d.features_sel],
            reg::QUEUE_NUM_MAX => 64,
            reg::QUEUE_READY => d.queues[d.queue_sel].ready,
            reg::STATUS => d.status,
            reg::CONFIG => {
                u32::from_le_bytes([0x52, 0x54, 0, 0x12, 0x34, 0x56][..4].try_into().unwrap())
            }
            o if o == reg::CONFIG + 4 => u32::from_le_bytes([0x34, 0x56, 0, 0]),
            _ => 0,
        }
    }

    fn write32(&mut self, offset: usize, value: u32) {
        let mut d = self.0.borrow_mut();
        let q = d.queue_sel;
        let set_low = |r: &mut u64, v: u32| *r = (*r & !0xffff_ffff) | u64::from(v);
        let set_high = |r: &mut u64, v: u32| *r = (*r & 0xffff_ffff) | u64::from(v) << 32;
        match offset {
            reg::DEVICE_FEATURES_SEL => d.features_sel = value as usize,
            reg::DRIVER_FEATURES_SEL => d.driver_sel = value as usize,
            reg::DRIVER_FEATURES => {
                let sel = d.driver_sel;
                d.driver[sel] = value;
            }
            reg::STATUS => d.set_status(value),
            reg::QUEUE_SEL => d.queue_sel = value as usize,
            reg::QUEUE_NUM => d.queues[q].num = value,
            reg::QUEUE_READY => d.queues[q].ready = value,
            reg::QUEUE_DESC_LOW => set_low(&mut d.queues[q].desc, value),
            reg::QUEUE_DESC_HIGH => set_high(&mut d.queues[q].desc, value),
            reg::QUEUE_DRIVER_LOW => set_low(&mut d.queues[q].avail, value),
            reg::QUEUE_DRIVER_HIGH => set_high(&mut d.queues[q].avail, value),
            reg::QUEUE_DEVICE_LOW => set_low(&mut d.queues[q].used, value),
            reg::QUEUE_DEVICE_HIGH => set_high(&mut d.queues[q].used, value),
            reg::QUEUE_NOTIFY => d.notify(value as usize),
            _ => {}
        }
    }

    fn read16(&mut self, _: usize) -> u16 {
        unreachable!("virtio-mmio registers are 32-bit")
    }

    fn write16(&mut self, _: usize, _: u16) {
        unreachable!("virtio-mmio registers are 32-bit")
    }

    fn read8(&mut self, _: usize) -> u8 {
        unreachable!("virtio-mmio registers are 32-bit")
    }

    fn write8(&mut self, _: usize, _: u8) {
        unreachable!("virtio-mmio registers are 32-bit")
    }

    fn barrier(&mut self) {}
}

/// The same device behind the virtio-pci structures: common configuration,
/// notifications (multiplier 4, queue `q` at `q * 4`) and device
/// configuration. Every access must have its register's width.
#[derive(Debug, Clone)]
enum PciRegs {
    Common(Rc<RefCell<Device>>),
    Notify(Rc<RefCell<Device>>),
    DeviceConfig,
}

const MAC: [u8; 6] = [0x52, 0x54, 0, 0x12, 0x34, 0x56];

impl DeviceRegisters for PciRegs {
    fn read32(&mut self, offset: usize) -> u32 {
        let Self::Common(d) = self else {
            panic!("32-bit read of {offset:#x} outside the common configuration");
        };
        let d = d.borrow();
        match offset {
            pci::common::DEVICE_FEATURE => d.features.get(d.features_sel).copied().unwrap_or(0),
            o => panic!("32-bit read of common register {o:#x}"),
        }
    }

    fn write32(&mut self, offset: usize, value: u32) {
        let Self::Common(d) = self else {
            panic!("32-bit write of {offset:#x} outside the common configuration");
        };
        let mut d = d.borrow_mut();
        let q = d.queue_sel;
        let set_low = |r: &mut u64, v: u32| *r = (*r & !0xffff_ffff) | u64::from(v);
        let set_high = |r: &mut u64, v: u32| *r = (*r & 0xffff_ffff) | u64::from(v) << 32;
        match offset {
            pci::common::DEVICE_FEATURE_SELECT => d.features_sel = value as usize,
            pci::common::DRIVER_FEATURE_SELECT => d.driver_sel = value as usize,
            pci::common::DRIVER_FEATURE => {
                let sel = d.driver_sel;
                d.driver[sel] = value;
            }
            o if o == pci::common::QUEUE_DESC => set_low(&mut d.queues[q].desc, value),
            o if o == pci::common::QUEUE_DESC + 4 => set_high(&mut d.queues[q].desc, value),
            o if o == pci::common::QUEUE_DRIVER => set_low(&mut d.queues[q].avail, value),
            o if o == pci::common::QUEUE_DRIVER + 4 => set_high(&mut d.queues[q].avail, value),
            o if o == pci::common::QUEUE_DEVICE => set_low(&mut d.queues[q].used, value),
            o if o == pci::common::QUEUE_DEVICE + 4 => set_high(&mut d.queues[q].used, value),
            o => panic!("32-bit write of common register {o:#x}"),
        }
    }

    fn read16(&mut self, offset: usize) -> u16 {
        let Self::Common(d) = self else {
            panic!("16-bit read of {offset:#x} outside the common configuration");
        };
        let d = d.borrow();
        let q = d.queues[d.queue_sel];
        match offset {
            // Queue 0 and 1 notify at offsets 0 and 1 (times the multiplier).
            pci::common::QUEUE_NOTIFY_OFF => d.queue_sel as u16,
            pci::common::QUEUE_ENABLE => q.ready as u16,
            pci::common::QUEUE_SIZE => {
                if q.num == 0 {
                    64
                } else {
                    q.num as u16
                }
            }
            o => panic!("16-bit read of common register {o:#x}"),
        }
    }

    fn write16(&mut self, offset: usize, value: u16) {
        match self {
            Self::Common(d) => {
                let mut d = d.borrow_mut();
                let q = d.queue_sel;
                match offset {
                    pci::common::QUEUE_SELECT => d.queue_sel = value.into(),
                    pci::common::QUEUE_SIZE => d.queues[q].num = value.into(),
                    pci::common::QUEUE_ENABLE => d.queues[q].ready = value.into(),
                    o => panic!("16-bit write of common register {o:#x}"),
                }
            }
            Self::Notify(d) => {
                assert_eq!(
                    offset,
                    value as usize * 4,
                    "queue {value} notified at {offset}"
                );
                d.borrow_mut().notify(value.into());
            }
            Self::DeviceConfig => panic!("16-bit write of device configuration"),
        }
    }

    fn read8(&mut self, offset: usize) -> u8 {
        match self {
            Self::Common(d) if offset == pci::common::DEVICE_STATUS => d.borrow().status as u8,
            Self::DeviceConfig => MAC[offset],
            _ => panic!("8-bit read of {offset:#x}"),
        }
    }

    fn write8(&mut self, offset: usize, value: u8) {
        match self {
            Self::Common(d) if offset == pci::common::DEVICE_STATUS => {
                let mut d = d.borrow_mut();
                d.set_status(value.into());
                if value == 0 {
                    d.queues = [QueueState::default(); 2];
                }
            }
            _ => panic!("8-bit write of {offset:#x}"),
        }
    }

    fn barrier(&mut self) {}
}

#[derive(Debug, Clone)]
struct Mem(Rc<RefCell<Device>>);

impl PhysicalMemory for Mem {
    fn read_u64(&self, phys: u64) -> u64 {
        self.0.borrow().mem.read_u64(phys)
    }
    fn write_u64(&mut self, phys: u64, value: u64) {
        self.0.borrow_mut().mem.write_u64(phys, value);
    }
}

fn frames() -> impl FnMut() -> Option<u64> {
    let mut next = 0x10_0000u64;
    move || {
        next += FRAME;
        Some(next)
    }
}

type Probed = Result<VirtioNet<Regs, Mem>, ProbeError>;

type PciProbed = Result<VirtioNet<PciRegs, Mem>, ProbeError>;

fn pci_card(device: Device) -> (PciProbed, Rc<RefCell<Device>>) {
    let shared = Rc::new(RefCell::new(device));
    let transport = pci::Transport::new(
        PciRegs::Common(shared.clone()),
        PciRegs::Notify(shared.clone()),
        4,
        PciRegs::DeviceConfig,
    );
    let card = VirtioNet::probe_pci(transport, Mem(shared.clone()), &mut frames());
    (card, shared)
}

fn card(device: Device) -> (Probed, Rc<RefCell<Device>>) {
    let shared = Rc::new(RefCell::new(device));
    let card = VirtioNet::probe(Regs(shared.clone()), Mem(shared.clone()), &mut frames());
    (card, shared)
}

#[test]
fn probe_negotiates_and_sets_up_both_queues() {
    let (card, dev) = card(Device::new());
    let card = card.unwrap();
    assert_eq!(card.mac(), [0x52, 0x54, 0, 0x12, 0x34, 0x56]);
    let d = dev.borrow();
    assert_eq!(d.status, ACKNOWLEDGE | DRIVER | FEATURES_OK | DRIVER_OK);
    assert_eq!(d.driver, [F_MAC, F_VERSION_1]);
    for q in d.queues {
        assert_eq!((q.num, q.ready), (u32::from(QUEUE_SIZE), 1));
        assert_eq!(q.desc % FRAME, 0);
    }
    assert_eq!(
        d.read16(d.queues[0].avail + 2),
        QUEUE_SIZE,
        "every receive buffer is offered"
    );
    assert_eq!(d.notifies, [1, 0]);
}

#[test]
fn frames_go_out_and_descriptors_come_back() {
    let (card, dev) = card(Device::new());
    let mut card = card.unwrap();
    for i in 0..100u8 {
        assert!(card.send(&[i; 60]));
    }
    let d = dev.borrow();
    assert_eq!(d.sent.len(), 100, "more frames than descriptors");
    assert_eq!(d.sent[99], [99; 60]);
    drop(d);
    assert!(!card.send(&vec![0; FRAME_MAX + 1]), "too large");
}

#[test]
fn received_frames_come_in_order_and_buffers_are_reused() {
    let (card, dev) = card(Device::new());
    let mut card = card.unwrap();
    let mut got = Vec::new();
    assert!(!card.recv(&mut |f| got.push(f.to_vec())));
    for round in 0..10u8 {
        for i in 0..20u8 {
            assert!(dev.borrow_mut().deliver(&[round, i, 1, 2, 3]));
        }
        while card.recv(&mut |f| got.push(f.to_vec())) {}
    }
    assert_eq!(got.len(), 200, "more frames than buffers");
    assert_eq!(got[199], [9, 19, 1, 2, 3]);
    assert_eq!(got[0], [0, 0, 1, 2, 3]);
}

#[test]
fn probe_refuses_what_it_cannot_drive() {
    let mut d = Device::new();
    d.version = 1;
    assert_eq!(card(d).0.err(), Some(ProbeError::Version(1)));
    let mut d = Device::new();
    d.id = 0;
    assert_eq!(card(d).0.err(), Some(ProbeError::Other(0)));
    let mut d = Device::new();
    d.features = [F_MAC, 0];
    assert_eq!(card(d).0.err(), Some(ProbeError::Features));
    let shared = Rc::new(RefCell::new(Device::new()));
    let mut none = || None;
    assert_eq!(
        VirtioNet::probe(Regs(shared.clone()), Mem(shared), &mut none).err(),
        Some(ProbeError::OutOfMemory)
    );
}

#[test]
fn without_a_mac_feature_the_address_is_local() {
    let mut d = Device::new();
    d.features = [0, F_VERSION_1];
    let card = card(d).0.unwrap();
    assert_eq!(card.mac(), [0x02, 0, 0, 0, 0, 1]);
}

#[test]
fn a_pci_card_negotiates_sends_and_receives() {
    let (card, dev) = pci_card(Device::new());
    let mut card = card.unwrap();
    assert_eq!(card.mac(), MAC);
    {
        let d = dev.borrow();
        assert_eq!(d.status, ACKNOWLEDGE | DRIVER | FEATURES_OK | DRIVER_OK);
        assert_eq!(d.driver, [F_MAC, F_VERSION_1]);
        for q in d.queues {
            assert_eq!((q.num, q.ready), (u32::from(QUEUE_SIZE), 1));
            assert_eq!((q.avail - q.desc, q.used - q.desc), (AVAIL_AT, USED_AT));
        }
        assert_eq!(d.notifies, [1, 0], "receive buffers offered");
    }
    for i in 0..40u8 {
        assert!(card.send(&[i; 60]));
    }
    assert_eq!(dev.borrow().sent.len(), 40);
    assert!(dev.borrow_mut().deliver(&[7; 64]));
    let mut got = Vec::new();
    assert!(card.recv(&mut |f| got.push(f.to_vec())));
    assert_eq!(got, [vec![7; 64]]);
}

#[test]
fn a_pci_card_is_refused_without_version_1() {
    let mut d = Device::new();
    d.features = [F_MAC, 0];
    assert_eq!(pci_card(d).0.err(), Some(ProbeError::Features));
}

#[test]
fn firecracker_gets_enough_receive_buffers() {
    let (card, dev) = card(Device::new());
    let mut card = card.unwrap();
    let (mut sent, mut got) = (0, 0);
    for _ in 0..10 {
        // A burst arrives before the kernel polls the card.
        let mut burst = 0;
        while burst < 64 && dev.borrow_mut().deliver_like_firecracker(&[7; 60]) {
            burst += 1;
        }
        assert!(burst >= 16, "a burst of {burst} frames only");
        sent += burst;
        while card.recv(&mut |f| {
            assert_eq!(f, [7; 60]);
            got += 1;
        }) {}
    }
    assert_eq!(got, sent, "every frame delivered is received");
}
