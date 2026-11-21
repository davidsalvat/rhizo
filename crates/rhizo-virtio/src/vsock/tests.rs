//! Host tests of the stream protocol alone, and of the device driver against
//! a simulated virtio-mmio socket device that reads and writes the rings in
//! fake physical memory, as a VMM would.

use super::*;
use crate::{ACKNOWLEDGE, DRIVER, DRIVER_OK, F_VERSION_1, FEATURES_OK, MAGIC, reg};
use proptest::prelude::*;
use rhizo_hal::testing::FakePhysicalMemory;
use std::cell::RefCell;
use std::rc::Rc;

const CID: u64 = 0x1_2345_6789;
const PORT: u32 = 1024;
const HOST_PORT: u32 = 77;

/// A packet from the host to the stream's port.
fn from_host(op: u16, len: u32) -> Header {
    Header {
        src_cid: HOST_CID,
        dst_cid: CID,
        src_port: HOST_PORT,
        dst_port: PORT,
        len,
        kind: STREAM,
        op,
        flags: 0,
        buf_alloc: 64 * 1024,
        fwd_cnt: 0,
    }
}

/// Every packet the stream has to send, with payloads of at most `max`.
fn drain(s: &mut Stream, max: usize) -> Vec<(Header, Vec<u8>)> {
    let mut out = Vec::new();
    let mut payload = Vec::new();
    while let Some(h) = s.next_packet(max, &mut payload) {
        out.push((h, payload.clone()));
    }
    out
}

fn read_all(s: &mut Stream) -> Vec<u8> {
    core::iter::from_fn(|| s.read()).collect()
}

fn connected() -> Stream {
    let mut s = Stream::new(CID, PORT);
    assert_eq!(
        s.take(&from_host(op::REQUEST, 0), &[]),
        Some(Change::Connected)
    );
    let sent = drain(&mut s, PAYLOAD_MAX);
    assert_eq!(sent.len(), 1);
    let (h, _) = sent[0];
    assert_eq!(h.op, op::RESPONSE);
    assert_eq!(
        (h.src_cid, h.src_port, h.dst_cid, h.dst_port),
        (CID, PORT, HOST_CID, HOST_PORT)
    );
    assert_eq!((h.kind, h.len, h.buf_alloc), (STREAM, 0, BUF_ALLOC));
    s
}

#[test]
fn headers_round_trip_little_endian() {
    let h = Header {
        src_cid: 0x0102_0304_0506_0708,
        dst_cid: 3,
        src_port: 0x1122_3344,
        dst_port: 5,
        len: 6,
        kind: STREAM,
        op: op::RW,
        flags: 3,
        buf_alloc: 9,
        fwd_cnt: 10,
    };
    let b = h.encode();
    assert_eq!(&b[..8], &[8, 7, 6, 5, 4, 3, 2, 1]);
    assert_eq!(&b[16..20], &[0x44, 0x33, 0x22, 0x11]);
    assert_eq!(&b[28..32], &[1, 0, 5, 0]);
    assert_eq!(Header::decode(&b), h);
}

#[test]
fn bytes_go_both_ways_once_connected() {
    let mut s = Stream::new(CID, PORT);
    assert!(!s.write(b"early"), "nothing is sent before a peer connects");
    let mut s2 = connected();
    assert!(s2.connected());
    assert_eq!(s2.take(&from_host(op::RW, 5), b"hello"), None);
    assert!(s2.has_input());
    assert_eq!(read_all(&mut s2), b"hello");
    assert!(!s2.has_input());
    assert!(s2.write(&[7; 5000]));
    let sent = drain(&mut s2, PAYLOAD_MAX);
    let lens: Vec<u32> = sent.iter().map(|(h, _)| h.len).collect();
    assert_eq!(
        lens,
        [2048, 2048, 904],
        "split into payloads of at most the buffer"
    );
    for (h, data) in &sent {
        assert_eq!((h.op, h.len as usize), (op::RW, data.len()));
        assert_eq!(h.fwd_cnt, 5, "every packet tells what was read");
    }
    assert!(drain(&mut s, PAYLOAD_MAX).is_empty());
}

#[test]
fn a_second_connection_and_strangers_get_resets() {
    let mut s = connected();
    let other = Header {
        src_port: HOST_PORT + 1,
        ..from_host(op::REQUEST, 0)
    };
    assert_eq!(s.take(&other, &[]), None);
    let sent = drain(&mut s, PAYLOAD_MAX);
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0].0.op, sent[0].0.dst_port), (op::RST, HOST_PORT + 1));
    // Data from a port with no connection.
    assert_eq!(s.take(&Header { len: 1, ..other }, b"x"), None);
    assert_eq!(drain(&mut s, PAYLOAD_MAX)[0].0.op, op::RST);
    // A reset is never answered.
    assert_eq!(
        s.take(
            &Header {
                op: op::RST,
                ..other
            },
            &[]
        ),
        None
    );
    assert!(drain(&mut s, PAYLOAD_MAX).is_empty());
    // The first connection is untouched.
    assert!(s.connected());
    assert_eq!(s.take(&from_host(op::RW, 1), b"y"), None);
    assert_eq!(read_all(&mut s), b"y");
}

#[test]
fn requests_to_another_port_or_type_are_refused() {
    let mut s = Stream::new(CID, PORT);
    for h in [
        Header {
            dst_port: PORT + 1,
            ..from_host(op::REQUEST, 0)
        },
        Header {
            dst_cid: CID + 1,
            ..from_host(op::REQUEST, 0)
        },
        Header {
            kind: 2,
            ..from_host(op::REQUEST, 0)
        },
    ] {
        assert_eq!(s.take(&h, &[]), None);
        assert!(!s.connected());
        let sent = drain(&mut s, PAYLOAD_MAX);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0.op, op::RST);
        assert_eq!(sent[0].0.src_port, h.dst_port);
    }
}

#[test]
fn shutdown_and_reset_close_and_a_new_connection_starts_afresh() {
    let mut s = connected();
    assert_eq!(s.take(&from_host(op::RW, 3), b"abc"), None);
    assert!(s.write(b"pending"));
    assert_eq!(
        s.take(
            &Header {
                flags: 3,
                ..from_host(op::SHUTDOWN, 0)
            },
            &[]
        ),
        Some(Change::Closed)
    );
    let sent = drain(&mut s, PAYLOAD_MAX);
    assert_eq!(sent.len(), 1, "a reset, and nothing pending");
    assert_eq!(sent[0].0.op, op::RST);
    assert_eq!(
        read_all(&mut s),
        b"",
        "nothing of the old connection is read"
    );
    let mut s = connected();
    assert_eq!(s.take(&from_host(op::RST, 0), &[]), Some(Change::Closed));
    assert!(
        drain(&mut s, PAYLOAD_MAX).is_empty(),
        "a reset is not answered"
    );
    let mut s = connected();
    assert!(s.write(b"new"));
    let sent = drain(&mut s, PAYLOAD_MAX);
    assert_eq!((sent[0].0.fwd_cnt, sent[0].1.as_slice()), (0, &b"new"[..]));
}

#[test]
fn a_peer_that_breaks_the_protocol_is_reset() {
    // More than the credit given.
    let mut s = connected();
    let big = vec![1u8; PAYLOAD_MAX];
    for _ in 0..BUF_ALLOC as usize / PAYLOAD_MAX {
        assert_eq!(s.take(&from_host(op::RW, PAYLOAD_MAX as u32), &big), None);
    }
    assert_eq!(s.take(&from_host(op::RW, 1), b"z"), Some(Change::Closed));
    assert_eq!(drain(&mut s, PAYLOAD_MAX)[0].0.op, op::RST);
    // A length that is not the payload's.
    let mut s = connected();
    assert_eq!(s.take(&from_host(op::RW, 9), b"abc"), Some(Change::Closed));
    // Another socket type on the connection.
    let mut s = connected();
    let h = Header {
        kind: 2,
        ..from_host(op::RW, 0)
    };
    assert_eq!(s.take(&h, &[]), Some(Change::Closed));
}

#[test]
fn unknown_operations_are_ignored() {
    let mut s = connected();
    for code in [0, op::REQUEST, op::RESPONSE, 8, 0xffff] {
        assert_eq!(s.take(&from_host(code, 0), &[]), None);
    }
    assert!(s.connected());
    assert!(drain(&mut s, PAYLOAD_MAX).is_empty());
}

#[test]
fn sending_waits_for_credit_and_asks_for_it_once() {
    let mut s = connected();
    let no_credit = Header {
        buf_alloc: 100,
        ..from_host(op::CREDIT_UPDATE, 0)
    };
    assert_eq!(s.take(&no_credit, &[]), None);
    assert!(s.write(&[1; 300]));
    let sent = drain(&mut s, PAYLOAD_MAX);
    let ops: Vec<u16> = sent.iter().map(|(h, _)| h.op).collect();
    assert_eq!(ops, [op::RW, op::CREDIT_REQUEST]);
    assert_eq!(sent[0].1.len(), 100);
    assert!(drain(&mut s, PAYLOAD_MAX).is_empty(), "asked once");
    // The peer read 100 bytes: room for 100 more.
    let more = Header {
        fwd_cnt: 100,
        ..no_credit
    };
    assert_eq!(s.take(&more, &[]), None);
    let sent = drain(&mut s, PAYLOAD_MAX);
    let ops: Vec<u16> = sent.iter().map(|(h, _)| h.op).collect();
    assert_eq!(ops, [op::RW, op::CREDIT_REQUEST]);
    assert_eq!(sent[0].1.len(), 100);
}

#[test]
fn credit_requests_are_answered_and_reading_returns_credit() {
    let mut s = connected();
    assert_eq!(s.take(&from_host(op::CREDIT_REQUEST, 0), &[]), None);
    let sent = drain(&mut s, PAYLOAD_MAX);
    assert_eq!(sent[0].0.op, op::CREDIT_UPDATE);
    let chunk = vec![0u8; PAYLOAD_MAX];
    let half = (BUF_ALLOC / 2) as usize;
    for _ in 0..half / PAYLOAD_MAX {
        s.take(&from_host(op::RW, PAYLOAD_MAX as u32), &chunk);
    }
    assert_eq!(read_all(&mut s).len(), half);
    let sent = drain(&mut s, PAYLOAD_MAX);
    assert_eq!(sent.len(), 1, "half the buffer read: the peer is told");
    assert_eq!(
        (sent[0].0.op, sent[0].0.fwd_cnt),
        (op::CREDIT_UPDATE, half as u32)
    );
}

#[test]
fn a_peer_that_never_reads_cannot_fill_memory() {
    let mut s = connected();
    let stuck = Header {
        buf_alloc: 0,
        ..from_host(op::CREDIT_UPDATE, 0)
    };
    s.take(&stuck, &[]);
    let block = vec![0u8; 1 << 20];
    for _ in 0..TX_MAX / block.len() {
        assert!(s.write(&block));
    }
    assert!(!s.write(b"x"), "over the limit");
    assert!(!s.connected());
    let ops: Vec<u16> = drain(&mut s, PAYLOAD_MAX)
        .iter()
        .map(|(h, _)| h.op)
        .collect();
    assert_eq!(ops, [op::RST]);
}

/// Headers with fields from the stream's own values or anything else.
fn any_header() -> impl Strategy<Value = Header> {
    let cid = prop_oneof![Just(CID), Just(HOST_CID), any::<u64>()];
    let port = prop_oneof![Just(PORT), Just(HOST_PORT), any::<u32>()];
    let small = prop_oneof![Just(0u32), 0..5000u32, any::<u32>()];
    (
        (cid.clone(), cid, port.clone(), port),
        (
            small.clone(),
            prop_oneof![Just(STREAM), any::<u16>()],
            0..10u16,
        ),
        (any::<u32>(), small.clone(), small),
    )
        .prop_map(
            |(
                (src_cid, dst_cid, src_port, dst_port),
                (len, kind, op),
                (flags, buf_alloc, fwd_cnt),
            )| {
                Header {
                    src_cid,
                    dst_cid,
                    src_port,
                    dst_port,
                    len,
                    kind,
                    op,
                    flags,
                    buf_alloc,
                    fwd_cnt,
                }
            },
        )
}

proptest! {
    /// Arbitrary packets, lengths, credit and operations, between writes
    /// and reads: nothing panics, at most the advertised buffer is ever
    /// held to read, and what is left to send stays bounded.
    #[test]
    fn hostile_packets_are_survived(
        steps in proptest::collection::vec(
            (any_header(), proptest::collection::vec(any::<u8>(), 0..3000), 0..3000usize, any::<bool>()),
            1..60,
        )
    ) {
        let mut s = connected();
        for (h, data, write, read) in steps {
            let data = if h.len as usize <= data.len() && read {
                &data[..h.len as usize]
            } else {
                &data[..]
            };
            s.take(&h, data);
            s.write(&vec![9u8; write]);
            if read {
                let held = read_all(&mut s).len();
                prop_assert!(held <= BUF_ALLOC as usize);
            }
            let sent = drain(&mut s, PAYLOAD_MAX);
            prop_assert!(sent.len() <= CONTROL_MAX + 3 + 3000 / PAYLOAD_MAX);
            for (h, payload) in sent {
                prop_assert_eq!(h.len as usize, payload.len());
                prop_assert!(payload.len() <= PAYLOAD_MAX);
            }
        }
    }

    /// Whatever credit the peer gives, overflowing counters included, the
    /// stream never sends more than it allows: its buffer less what was
    /// sent and it has not read.
    #[test]
    fn credit_is_never_exceeded(
        credits in proptest::collection::vec((any::<u32>(), any::<u32>(), 0..20_000usize), 1..30)
    ) {
        let mut s = connected();
        let mut sent: u32 = 0;
        for (buf_alloc, fwd_cnt, write) in credits {
            let update = Header { buf_alloc, fwd_cnt, ..from_host(op::CREDIT_UPDATE, 0) };
            s.take(&update, &[]);
            s.write(&vec![1u8; write]);
            let allowed = buf_alloc.saturating_sub(sent.wrapping_sub(fwd_cnt));
            let mut out = 0u32;
            for (h, payload) in drain(&mut s, PAYLOAD_MAX) {
                if h.op == op::RW {
                    out += payload.len() as u32;
                }
            }
            prop_assert!(out <= allowed, "{} sent with {} of credit", out, allowed);
            sent = sent.wrapping_add(out);
        }
    }
}

// The driver against a simulated device.

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

#[derive(Debug, Default)]
struct Device {
    mem: FakePhysicalMemory,
    features: [u32; 2],
    driver: [u32; 2],
    features_sel: usize,
    driver_sel: usize,
    status: u32,
    queue_sel: usize,
    queues: [QueueState; 3],
    /// Packets the driver sent.
    sent: Vec<(Header, Vec<u8>)>,
    notifies: [usize; 3],
    /// Whether the device takes what the driver sends when notified.
    stalled: bool,
}

/// A descriptor: address, length, flags.
type Desc = (u64, u32, u16);

impl Device {
    fn new() -> Self {
        Self {
            features: [F_STREAM, F_VERSION_1],
            ..Self::default()
        }
    }

    fn read16(&self, at: u64) -> u16 {
        let mut b = [0u8; 2];
        self.mem.read_bytes(at, &mut b);
        u16::from_le_bytes(b)
    }

    /// The next chain the driver offered on queue `q`: its head and
    /// descriptors.
    fn next_chain(&mut self, q: usize) -> Option<(u16, Vec<Desc>)> {
        let s = self.queues[q];
        if self.read16(s.avail + 2) == s.next {
            return None;
        }
        let head = self.read16(s.avail + 4 + u64::from(s.next % s.num as u16) * 2);
        self.queues[q].next = s.next.wrapping_add(1);
        let mut chain = Vec::new();
        let mut id = head;
        loop {
            let mut d = [0u8; 16];
            self.mem.read_bytes(s.desc + u64::from(id) * 16, &mut d);
            let addr = u64::from_le_bytes(d[..8].try_into().unwrap());
            let len = u32::from_le_bytes(d[8..12].try_into().unwrap());
            let flags = u16::from_le_bytes([d[12], d[13]]);
            chain.push((addr, len, flags));
            if flags & 1 == 0 || chain.len() > 4 {
                break;
            }
            id = u16::from_le_bytes([d[14], d[15]]);
        }
        Some((head, chain))
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

    /// Queue `q` was notified: the transmit queue sends what was offered.
    fn notify(&mut self, q: usize) {
        self.notifies[q] += 1;
        while q == 1
            && !self.stalled
            && let Some((head, chain)) = self.next_chain(1)
        {
            let mut h = [0u8; HEADER];
            assert_eq!(chain[0].1 as usize, HEADER, "the header alone first");
            self.mem.read_bytes(chain[0].0, &mut h);
            let h = Header::decode(&h);
            let mut data = vec![0u8; h.len as usize];
            if h.len > 0 {
                assert_eq!(chain.len(), 2);
                assert_eq!(chain[1].1, h.len);
                self.mem.read_bytes(chain[1].0, &mut data);
            } else {
                assert_eq!(chain.len(), 1, "no payload descriptor");
            }
            self.sent.push((h, data));
            self.give_back(1, head, 0);
        }
    }

    /// Writes `header` and `payload` into the next receive chain and gives
    /// it back saying `written` bytes; `false` if none is offered.
    fn deliver_raw(&mut self, header: &[u8; HEADER], payload: &[u8], written: u32) -> bool {
        let Some((head, chain)) = self.next_chain(0) else {
            return false;
        };
        assert_eq!(chain.len(), 2, "a header and a payload buffer");
        assert_eq!(chain[0].1 as usize, HEADER);
        assert!(chain.iter().all(|d| d.2 & DESC_F_WRITE != 0));
        assert!(payload.len() <= chain[1].1 as usize);
        self.mem.write_bytes(chain[0].0, header);
        self.mem.write_bytes(chain[1].0, payload);
        self.give_back(0, head, written);
        true
    }

    fn deliver(&mut self, h: &Header, payload: &[u8]) -> bool {
        self.deliver_raw(&h.encode(), payload, (HEADER + payload.len()) as u32)
    }
}

#[derive(Debug, Clone)]
struct Regs(Rc<RefCell<Device>>);

impl DeviceRegisters for Regs {
    fn read32(&mut self, offset: usize) -> u32 {
        let d = self.0.borrow();
        match offset {
            reg::MAGIC => MAGIC,
            reg::VERSION => 2,
            reg::DEVICE_ID => VSOCK_DEVICE,
            reg::DEVICE_FEATURES => d.features[d.features_sel],
            reg::QUEUE_NUM_MAX => 128,
            reg::QUEUE_READY => d.queues[d.queue_sel].ready,
            reg::STATUS => d.status,
            reg::CONFIG => CID as u32,
            o if o == reg::CONFIG + 4 => (CID >> 32) as u32,
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
            reg::STATUS => d.status = value,
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

type Vsock = VirtioVsock<Regs, Mem>;

fn device() -> (Vsock, Rc<RefCell<Device>>) {
    let shared = Rc::new(RefCell::new(Device::new()));
    let mut next = 0x10_0000u64;
    let mut taken = 0;
    let mut frames = || {
        next += crate::FRAME;
        taken += 1;
        Some(next)
    };
    let dev = VirtioVsock::probe(Regs(shared.clone()), Mem(shared.clone()), &mut frames).unwrap();
    assert_eq!(taken, FRAMES);
    (dev, shared)
}

/// Moves every packet the device received into `s`, then everything `s`
/// has to send out through the device.
fn pump(dev: &mut Vsock, s: &mut Stream) -> Vec<Change> {
    let mut changes = Vec::new();
    while dev.recv(&mut |h, data| changes.extend(s.take(h, data))) {}
    let mut payload = Vec::new();
    while dev.can_send()
        && let Some(h) = s.next_packet(PAYLOAD_MAX, &mut payload)
    {
        assert!(dev.send(&h, &payload));
    }
    changes
}

#[test]
fn probe_negotiates_and_sets_up_three_queues() {
    let (dev, d) = device();
    assert_eq!(dev.cid(), CID);
    let d = d.borrow();
    assert_eq!(d.status, ACKNOWLEDGE | DRIVER | FEATURES_OK | DRIVER_OK);
    assert_eq!(d.driver, [F_STREAM, F_VERSION_1]);
    assert!(
        d.queues
            .iter()
            .all(|q| q.ready == 1 && q.num == u32::from(QUEUE_SIZE))
    );
    assert_eq!(
        d.read16(d.queues[0].avail + 2),
        SLOTS,
        "every receive slot offered"
    );
    assert_eq!(d.read16(d.queues[2].avail + 2), 0, "no event buffers");
}

#[test]
fn probe_refuses_another_device() {
    struct Net;
    impl DeviceRegisters for Net {
        fn read32(&mut self, offset: usize) -> u32 {
            match offset {
                reg::MAGIC => MAGIC,
                reg::VERSION => 2,
                reg::DEVICE_ID => crate::NET_DEVICE,
                _ => 0,
            }
        }
        fn write32(&mut self, _: usize, _: u32) {}
        fn read16(&mut self, _: usize) -> u16 {
            0
        }
        fn write16(&mut self, _: usize, _: u16) {}
        fn read8(&mut self, _: usize) -> u8 {
            0
        }
        fn write8(&mut self, _: usize, _: u8) {}
        fn barrier(&mut self) {}
    }
    let r = VirtioVsock::probe(Net, FakePhysicalMemory::new(), &mut || Some(0x1000));
    assert_eq!(r.err(), Some(ProbeError::Other(crate::NET_DEVICE)));
}

#[test]
fn a_connection_carries_bytes_through_the_device() {
    let (mut dev, d) = device();
    let mut s = Stream::new(dev.cid(), PORT);
    assert!(d.borrow_mut().deliver(&from_host(op::REQUEST, 0), &[]));
    assert_eq!(pump(&mut dev, &mut s), [Change::Connected]);
    assert!(d.borrow_mut().deliver(&from_host(op::RW, 4), b"ping"));
    pump(&mut dev, &mut s);
    assert_eq!(read_all(&mut s), b"ping");
    assert!(s.write(&[5; 3000]));
    pump(&mut dev, &mut s);
    let d = d.borrow();
    let ops: Vec<(u16, usize)> = d.sent.iter().map(|(h, p)| (h.op, p.len())).collect();
    assert_eq!(ops, [(op::RESPONSE, 0), (op::RW, 2048), (op::RW, 952)]);
    assert_eq!(d.sent[1].0.dst_port, HOST_PORT);
    assert_eq!(d.sent[2].0.fwd_cnt, 4);
    assert_eq!(d.notifies[1], 3, "one notification per packet sent");
}

#[test]
fn slots_are_reused_and_a_full_queue_refuses() {
    let (mut dev, d) = device();
    let h = from_host(op::RW, 1);
    for _ in 0..500 {
        assert!(dev.send(&h, b"x"));
        assert!(d.borrow_mut().deliver(&h, b"y"));
        assert!(dev.recv(&mut |_, data| assert_eq!(data, b"y")));
    }
    d.borrow_mut().stalled = true;
    for _ in 0..SLOTS {
        assert!(dev.send(&h, b"x"));
    }
    assert!(!dev.can_send());
    assert!(!dev.send(&h, b"x"), "every slot in flight");
    d.borrow_mut().stalled = false;
    d.borrow_mut().notify(1);
    assert!(dev.can_send(), "slots come back once the device took them");
    assert!(!dev.send(&h, &[0; PAYLOAD_MAX + 1]), "too large");
}

#[test]
fn a_hostile_device_cannot_overrun_buffers() {
    let (mut dev, d) = device();
    // Shorter than a header: skipped.
    assert!(d.borrow_mut().deliver_raw(&[0xff; HEADER], &[], 10));
    assert!(dev.recv(&mut |_, _| panic!("not a packet")));
    // A length beyond what was written, and beyond the buffer.
    let h = Header {
        len: u32::MAX,
        ..from_host(op::RW, 0)
    };
    assert!(d.borrow_mut().deliver_raw(&h.encode(), b"abc", u32::MAX));
    let mut got = 0;
    assert!(dev.recv(&mut |h, data| {
        assert_eq!(h.len, u32::MAX);
        got = data.len();
    }));
    assert_eq!(got, PAYLOAD_MAX, "clipped to the buffer");
    // A used entry naming a descriptor out of range or in the middle of a
    // chain.
    for id in [u16::MAX, 3] {
        let mut dd = d.borrow_mut();
        dd.next_chain(0);
        dd.give_back(0, id, HEADER as u32);
        drop(dd);
        assert!(dev.recv(&mut |_, _| {}));
    }
    // Nothing is left to read, and the slots still work.
    assert!(!dev.recv(&mut |_, _| {}));
    assert!(d.borrow_mut().deliver(&from_host(op::RW, 2), b"ok"));
    assert!(dev.recv(&mut |_, data| assert_eq!(data, b"ok")));
}
