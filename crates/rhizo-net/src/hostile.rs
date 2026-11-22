//! Frames from the network as hostile input (change `fuzz-wire-packets`,
//! boundary B5 of `docs/audit-scope.md`): a [`Net`] on a [`QueueLink`]
//! with two cells' sockets, a well-behaved peer (10.0.0.1) talking to one
//! of them, and an attacker (10.0.0.66) that sends anything: valid frames
//! (SYN to a listening port, data on its own connections, ARP, DHCP
//! replies), the same with bytes changed (checksums fixed or not) and cut,
//! its own stack's frames replayed and changed, random bytes and floods.
//!
//! What it may not do: panic the kernel, open a socket, disturb the other
//! cell's connection or loopback sockets, see their bytes, or leave memory
//! behind once every socket is closed. Frames that claim to be the peer
//! (its MAC address or its IP address) are not sent: without
//! authentication a host that can impersonate the peer may reset or
//! redirect that connection on any TCP/IP stack (design D3).

use super::wire_tests::{ME, MS, PEER, Peer};
use super::*;
use proptest::prelude::*;
use rhizo_hal::testing::QueueLink;
use smoltcp::wire::{Icmpv4Packet, IpAddress, IpProtocol, Ipv4Packet, TcpPacket, UdpPacket};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

/// Heap bytes this thread holds: the test binary's allocator counts them
/// per thread, so tests running at once do not disturb each other.
struct Counting;

std::thread_local! {
    static LIVE: Cell<isize> = const { Cell::new(0) };
}

fn track(delta: isize) {
    let _ = LIVE.try_with(|l| l.set(l.get() + delta));
}

fn live() -> isize {
    LIVE.with(Cell::get)
}

// SAFETY: every call goes to `System` unchanged; this only counts sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller keeps `alloc`'s contract, passed on as is.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            track(layout.size() as isize);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: as `alloc`.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            track(layout.size() as isize);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        track(-(layout.size() as isize));
        // SAFETY: `ptr` came from `System` with `layout` (this allocator).
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new: usize) -> *mut u8 {
        // SAFETY: as `dealloc`, and the caller keeps `realloc`'s contract.
        let p = unsafe { System.realloc(ptr, layout, new) };
        if !p.is_null() {
            track(new as isize - layout.size() as isize);
        }
        p
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

const NET_MAC: [u8; 6] = [2, 0, 0, 0, 0, 2];
const PEER_MAC: [u8; 6] = [2, 0, 0, 0, 0, 1];
const EVIL_MAC: [u8; 6] = [2, 0, 0, 0, 0, 0x66];
const EVIL: [u8; 4] = [10, 0, 0, 66];
const ALL: [u8; 6] = [0xff; 6];
/// The attacker's port nobody listens on: the test answers the card's
/// SYNs to it with crafted segments.
const CRAFTED: u16 = 9000;
const CRAFTED_ISN: u32 = 1000;
const SECRET: &[u8] = b"A-SECRET";
const LOOPBACK_SECRET: &[u8] = b"LOOPBACK-SECRET";

const SYN: u8 = 0x02;
const ACK: u8 = 0x10;

fn eth(dst: [u8; 6], ty: u16, payload: &[u8]) -> Vec<u8> {
    [&dst[..], &EVIL_MAC, &ty.to_be_bytes(), payload].concat()
}

/// An IPv4 packet: `frag` holds the flags and fragment offset, `opts` a
/// multiple of 4 bytes.
fn ipv4(src: [u8; 4], dst: [u8; 4], proto: u8, frag: u16, opts: &[u8], data: &[u8]) -> Vec<u8> {
    let len = 20 + opts.len();
    let mut p = vec![0u8; len];
    p[0] = 0x40 | (len / 4) as u8;
    p[2..4].copy_from_slice(&((len + data.len()) as u16).to_be_bytes());
    p[6..8].copy_from_slice(&frag.to_be_bytes());
    p[8] = 64;
    p[9] = proto;
    p[12..16].copy_from_slice(&src);
    p[16..20].copy_from_slice(&dst);
    p[20..].copy_from_slice(opts);
    p.extend_from_slice(data);
    p
}

fn tcp_segment(
    ports: (u16, u16),
    seq: u32,
    ack: u32,
    flags: u8,
    opts: &[u8],
    data: &[u8],
) -> Vec<u8> {
    let len = (20 + opts.len()).next_multiple_of(4);
    let mut t = vec![0u8; len];
    t[0..2].copy_from_slice(&ports.0.to_be_bytes());
    t[2..4].copy_from_slice(&ports.1.to_be_bytes());
    t[4..8].copy_from_slice(&seq.to_be_bytes());
    t[8..12].copy_from_slice(&ack.to_be_bytes());
    t[12] = ((len / 4) << 4) as u8;
    t[13] = flags;
    t[14..16].copy_from_slice(&0xffffu16.to_be_bytes());
    t[20..20 + opts.len()].copy_from_slice(opts);
    t.extend_from_slice(data);
    t
}

fn udp(sport: u16, dport: u16, data: &[u8]) -> Vec<u8> {
    let len = (8 + data.len()) as u16;
    [
        &sport.to_be_bytes()[..],
        &dport.to_be_bytes(),
        &len.to_be_bytes(),
        &[0, 0],
        data,
    ]
    .concat()
}

fn arp(op: u16, sender: [u8; 4], target_mac: [u8; 6], target: [u8; 4]) -> Vec<u8> {
    let head = [0, 1, 8, 0, 6, 4];
    [
        &head[..],
        &op.to_be_bytes(),
        &EVIL_MAC,
        &sender,
        &target_mac,
        &target,
    ]
    .concat()
}

/// A DHCP reply of type `kind` from the attacker for transaction `xid`.
fn dhcp(kind: u8, xid: [u8; 4], yiaddr: [u8; 4], lease: u32) -> Vec<u8> {
    let mut b = vec![0u8; 240];
    b[..3].copy_from_slice(&[2, 1, 6]);
    b[4..8].copy_from_slice(&xid);
    b[16..20].copy_from_slice(&yiaddr);
    b[20..24].copy_from_slice(&EVIL);
    b[28..34].copy_from_slice(&NET_MAC);
    b[236..240].copy_from_slice(&[99, 130, 83, 99]);
    b.extend_from_slice(&[53, 1, kind, 54, 4]);
    b.extend_from_slice(&EVIL);
    b.extend_from_slice(&[
        1, 4, 255, 255, 255, 0, 3, 4, 10, 0, 0, 66, 6, 8, 10, 0, 0, 66, 8, 8, 8, 8,
    ]);
    for (code, secs) in [(51, lease), (58, lease / 2), (59, lease / 8 * 7)] {
        b.extend_from_slice(&[code, 4]);
        b.extend_from_slice(&secs.to_be_bytes());
    }
    b.push(255);
    let ip = ipv4(EVIL, [255; 4], 17, 0, &[], &udp(67, 68, &b));
    eth(ALL, 0x0800, &ip)
}

/// Sets the IPv4 header checksum and the TCP, UDP or ICMP one of `f`, when
/// its headers hold together enough to say where they are.
fn fix_checksums(f: &mut [u8]) {
    if f.len() < 14 || f[12..14] != [8, 0] {
        return;
    }
    let Ok(mut ip) = Ipv4Packet::new_checked(&mut f[14..]) else {
        return;
    };
    ip.fill_checksum();
    let (src, dst) = (
        IpAddress::Ipv4(ip.src_addr()),
        IpAddress::Ipv4(ip.dst_addr()),
    );
    let proto = ip.next_header();
    let data = ip.payload_mut();
    match proto {
        IpProtocol::Tcp => {
            if let Ok(mut t) = TcpPacket::new_checked(data) {
                t.fill_checksum(&src, &dst);
            }
        }
        IpProtocol::Udp => {
            if let Ok(mut u) = UdpPacket::new_checked(data) {
                u.fill_checksum(&src, &dst);
            }
        }
        IpProtocol::Icmp => {
            if let Ok(mut i) = Icmpv4Packet::new_checked(data) {
                i.fill_checksum();
            }
        }
        _ => {}
    }
}

/// Whether `f` claims to come from the well-behaved peer (design D3).
fn claims_the_peer(f: &[u8]) -> bool {
    let ip_src = |at: usize| f.get(at..at + 4) == Some(&PEER[..]);
    f.get(6..12) == Some(&PEER_MAC[..])
        || (f.get(12..14) == Some(&[8, 0]) && ip_src(26))
        || (f.get(12..14) == Some(&[8, 6]) && ip_src(28))
}

/// Source port, destination port, sequence number and flags of a TCP
/// segment in an IPv4 frame.
fn tcp_of(f: &[u8]) -> Option<(u16, u16, u32, u8)> {
    if f.len() < 34 || f[12..14] != [8, 0] || f[23] != 6 {
        return None;
    }
    let at = 14 + usize::from(f[14] & 0xf) * 4;
    let t = f.get(at..at + 20)?;
    let be16 = |i: usize| u16::from_be_bytes([t[i], t[i + 1]]);
    let seq = u32::from_be_bytes([t[4], t[5], t[6], t[7]]);
    Some((be16(0), be16(2), seq, t[13]))
}

fn holds(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Changes to a frame: bytes set (mostly in the first 80, where the headers
/// are), maybe a cut, then checksums fixed or not.
#[derive(Debug, Clone)]
struct Mutation {
    edits: Vec<(u16, u8)>,
    cut: Option<u16>,
    fix: bool,
}

impl Mutation {
    fn apply(&self, f: &mut Vec<u8>) {
        for &(at, b) in &self.edits {
            if !f.is_empty() {
                let span = if at & 0x8000 == 0 {
                    f.len().min(80)
                } else {
                    f.len()
                };
                f[usize::from(at) % span] = b;
            }
        }
        if let Some(cut) = self.cut {
            f.truncate(usize::from(cut) % (f.len() + 1));
        }
        if self.fix {
            fix_checksums(f);
        }
    }
}

fn mutation() -> impl Strategy<Value = Mutation> {
    (
        prop::collection::vec((any::<u16>(), any::<u8>()), 0..4),
        prop::option::weighted(0.1, any::<u16>()),
        prop::bool::weighted(0.85),
    )
        .prop_map(|(edits, cut, fix)| Mutation { edits, cut, fix })
}

/// Valid frames the attacker starts from.
#[derive(Debug, Clone, Copy)]
enum Seed {
    SynToListener,
    SynToClosedPort,
    SynToLoopback,
    SynWithOddOptions,
    SegmentToOwnConnection,
    SegmentToOtherCellsPort,
    SynAckOnCrafted,
    DataOnCrafted,
    UdpToOwnSocket,
    UdpToClosedPort,
    UdpBroadcast,
    Jumbo,
    FirstFragment,
    LaterFragment,
    IpOptions,
    IcmpEcho,
    IcmpUnreachable,
    ArpRequest,
    ArpReply,
    DhcpOffer,
    OtherEthertype,
}

const SEEDS: [Seed; 21] = [
    Seed::SynToListener,
    Seed::SynToClosedPort,
    Seed::SynToLoopback,
    Seed::SynWithOddOptions,
    Seed::SegmentToOwnConnection,
    Seed::SegmentToOtherCellsPort,
    Seed::SynAckOnCrafted,
    Seed::DataOnCrafted,
    Seed::UdpToOwnSocket,
    Seed::UdpToClosedPort,
    Seed::UdpBroadcast,
    Seed::Jumbo,
    Seed::FirstFragment,
    Seed::LaterFragment,
    Seed::IpOptions,
    Seed::IcmpEcho,
    Seed::IcmpUnreachable,
    Seed::ArpRequest,
    Seed::ArpReply,
    Seed::DhcpOffer,
    Seed::OtherEthertype,
];

/// TCP options: the usual SYN set, then odd ones (short, too long, a zero
/// MSS, a window scale past 14, SACK without ranges, unknown kinds).
const OPTIONS: [&[u8]; 12] = [
    &[
        2, 4, 5, 0xb4, 1, 3, 3, 7, 4, 2, 8, 10, 0, 0, 0, 1, 0, 0, 0, 0,
    ],
    &[2, 1],
    &[2, 4, 0, 0],
    &[2, 4, 0, 1],
    &[3, 3, 200],
    &[3, 2],
    &[5, 2],
    &[5, 10, 0, 0, 0, 9, 0, 0, 0, 1],
    &[8, 3, 0],
    &[0xfe, 0],
    &[2, 40],
    &[1; 40],
];

fn bytes(n: u32, len: usize) -> Vec<u8> {
    (0..len).map(|i| (n as usize + i * 7) as u8).collect()
}

#[derive(Debug, Clone)]
enum Step {
    /// Frames cross the wire for this many milliseconds.
    Run(u8),
    /// This many seconds pass, 50 ms at a time (retransmissions, timeouts).
    Wait(u8),
    /// The cells send on their sockets.
    Talk,
    Seed(Seed, u32, Mutation),
    /// A frame the attacker's stack sent, replayed with changes.
    Replay(u8, Mutation),
    /// Random bytes, with Ethernet addresses and type made to reach IPv4
    /// when `ip`.
    Raw(Vec<u8>, bool),
    /// 8 frames per unit at once: SYNs to the listener and datagrams.
    Flood(u8),
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        3 => any::<u8>().prop_map(Step::Run),
        1 => (0u8..30).prop_map(Step::Wait),
        2 => Just(Step::Talk),
        8 => (prop::sample::select(SEEDS.to_vec()), any::<u32>(), mutation())
            .prop_map(|(s, n, m)| Step::Seed(s, n, m)),
        3 => (any::<u8>(), mutation()).prop_map(|(i, m)| Step::Replay(i, m)),
        2 => (prop::collection::vec(any::<u8>(), 0..1600), any::<bool>())
            .prop_map(|(b, ip)| Step::Raw(b, ip)),
        1 => any::<u8>().prop_map(Step::Flood),
    ]
}

/// The sockets of the two cells. Cell A (the victim): `a`, a connection to
/// the peer, and a loopback connection `lc` accepted as `ls` from `la`.
/// Cell B (the attacker's): `b` to its port 80, `c` to [`CRAFTED`], `u` a
/// UDP socket that sent to it, `l` listening on port 8080.
#[derive(Debug, Clone, Copy)]
struct Sockets {
    a: Handle,
    la: Handle,
    lc: Handle,
    ls: Handle,
    b: Handle,
    c: Handle,
    u: Handle,
    l: Handle,
}

impl Sockets {
    fn all(&self) -> [Handle; 8] {
        [
            self.a, self.la, self.lc, self.ls, self.b, self.c, self.u, self.l,
        ]
    }
}

struct World {
    net: Net,
    link: QueueLink,
    peer: Peer,
    evil: Peer,
    now: u64,
    /// The attacker's own stack's frames, most recent last.
    captured: Vec<Vec<u8>>,
    /// The card's port and initial sequence number on [`CRAFTED`].
    crafted: Option<(u16, u32)>,
    /// Bytes cell A sent to the peer and got back.
    sent: Vec<u8>,
    got: Vec<u8>,
    talks: u32,
}

impl World {
    fn new() -> Self {
        let mut link = QueueLink {
            mac: NET_MAC,
            ..QueueLink::default()
        };
        let mut net = Net::new();
        let config = WireConfig::Static {
            addr: ME,
            prefix: 24,
            gateway: None,
        };
        net.attach(&mut link, config, 0, 7);
        Self {
            net,
            link,
            peer: Peer::at(PEER_MAC, PEER),
            evil: Peer::at(EVIL_MAC, EVIL),
            now: 0,
            captured: Vec::new(),
            crafted: None,
            sent: Vec::new(),
            got: Vec::new(),
            talks: 0,
        }
    }

    fn open(&mut self) -> Sockets {
        let net = &mut self.net;
        let stream = |net: &mut Net| net.socket(Family::Inet, Type::Stream).unwrap();
        let (a, b, c) = (stream(net), stream(net), stream(net));
        for (h, ip, port) in [(a, PEER, 80), (b, EVIL, 80), (c, EVIL, CRAFTED)] {
            assert_eq!(
                net.connect(h, &Addr::Inet { ip, port }),
                Err(Error::InProgress)
            );
        }
        let u = net.socket(Family::Inet, Type::Dgram).unwrap();
        net.send(u, b"hello", Some(&Addr::Inet { ip: EVIL, port: 53 }))
            .unwrap();
        let l = stream(net);
        net.bind(
            l,
            &Addr::Inet {
                ip: ANY,
                port: 8080,
            },
        )
        .unwrap();
        net.listen(l, 16).unwrap();
        let la = stream(net);
        let local = Addr::Inet {
            ip: LOOPBACK,
            port: 7000,
        };
        net.bind(la, &local).unwrap();
        net.listen(la, 16).unwrap();
        let lc = stream(net);
        net.connect(lc, &local).unwrap();
        let ls = net.accept(la).unwrap();
        net.send(lc, b"LOOPBACK-SECRET-1", None).unwrap();
        net.send(ls, b"LOOPBACK-SECRET-2", None).unwrap();
        net.end_call(|| None);
        let s = Sockets {
            a,
            la,
            lc,
            ls,
            b,
            c,
            u,
            l,
        };
        // `smoltcp` sends one ARP request a second: the attacker's address
        // is known after the peer's.
        self.run(1500, 10, &s);
        for (h, ip) in [(a, PEER), (b, EVIL)] {
            assert_eq!(self.net.connect(h, &Addr::Inet { ip, port: 80 }), Ok(()));
        }
        assert!(self.crafted.is_some(), "a SYN to the crafted port");
        self.net.end_call(|| None);
        s
    }

    /// Delivers what the card sent, checking that cell A's bytes go to the
    /// peer only and loopback bytes nowhere.
    fn route(&mut self) {
        while let Some(f) = self.link.sent.pop_front() {
            assert!(!holds(&f, LOOPBACK_SECRET), "loopback bytes on the wire");
            let dst: [u8; 6] = f[..6].try_into().unwrap();
            if dst == PEER_MAC {
                self.peer.link.inbox.push_back(f);
                continue;
            }
            assert!(!holds(&f, SECRET), "cell A's bytes sent to {dst:x?}");
            if dst == ALL {
                self.peer.link.inbox.push_back(f.clone());
            }
            match tcp_of(&f) {
                Some((port, CRAFTED, seq, flags)) if flags & SYN != 0 => {
                    self.crafted = Some((port, seq));
                }
                Some((_, CRAFTED, ..)) => {}
                _ if dst == ALL || dst == EVIL_MAC => self.evil.link.inbox.push_back(f),
                _ => {}
            }
        }
    }

    /// The card, both peers, then the card again.
    fn exchange(&mut self) {
        self.net.poll_wire(&mut self.link, self.now);
        self.route();
        self.peer.step(self.now);
        self.evil.step(self.now);
        self.link.inbox.extend(self.peer.link.sent.drain(..));
        for f in self.evil.link.sent.drain(..) {
            if self.captured.len() == 32 {
                self.captured.remove(0);
            }
            self.captured.push(f.clone());
            self.link.inbox.push_back(f);
        }
        self.net.poll_wire(&mut self.link, self.now);
        self.route();
    }

    /// What the cells read: cell A keeps what it gets.
    fn read(&mut self, s: &Sockets) {
        let net = &mut self.net;
        let got = &mut self.got;
        match net.recv(s.a, usize::MAX, false, &mut |b| {
            got.extend_from_slice(b);
            true
        }) {
            Ok(_) | Err(Error::Again) => {}
            Err(e) => panic!("cell A's connection failed: {e:?}"),
        }
        for h in [s.b, s.c, s.u] {
            let _ = net.recv(h, usize::MAX, false, &mut |_| true);
        }
        assert_eq!(net.accept(s.l), Err(Error::Again), "nothing connects");
        net.end_call(|| None);
    }

    fn run(&mut self, ms: u64, step: u64, s: &Sockets) {
        for _ in 0..ms / step {
            self.now += step * MS;
            self.exchange();
            self.read(s);
        }
    }

    fn talk(&mut self, s: &Sockets) {
        self.talks += 1;
        let msg = format!("A-SECRET:{};", self.talks);
        if self.net.poll(s.a) & events::OUT != 0 {
            let n = self.net.send(s.a, msg.as_bytes(), None).unwrap();
            self.sent.extend_from_slice(&msg.as_bytes()[..n]);
        }
        for h in [s.b, s.c] {
            let _ = self.net.send(h, b"from cell B", None);
        }
        let _ = self
            .net
            .send(s.u, b"datagram", Some(&Addr::Inet { ip: EVIL, port: 53 }));
        self.net.end_call(|| None);
    }

    /// The port `h` has on the wire: its card port, which the outside
    /// sees and its cell does not (design D6 of `add-cell-hierarchy`).
    fn port(&self, h: Handle) -> u16 {
        self.net.card_of(h)
    }

    fn seed(&self, seed: Seed, n: u32, s: &Sockets) -> Vec<u8> {
        let ip =
            |dst, proto, data: &[u8]| eth(NET_MAC, 0x0800, &ipv4(EVIL, dst, proto, 0, &[], data));
        let sport = 1024 + (n % 50_000) as u16;
        let opts = OPTIONS[n as usize % OPTIONS.len()];
        let data = bytes(n, (n % 200) as usize);
        let (crafted_port, their_seq) = self.crafted.unwrap_or((0, 0));
        let mut f = match seed {
            Seed::SynToListener => ip(
                ME,
                6,
                &tcp_segment((sport, 8080), n, 0, SYN, OPTIONS[0], &[]),
            ),
            Seed::SynToClosedPort => {
                ip(ME, 6, &tcp_segment((sport, n as u16), n, 0, SYN, &[], &[]))
            }
            Seed::SynToLoopback => eth(
                NET_MAC,
                0x0800,
                &ipv4(
                    EVIL,
                    LOOPBACK,
                    6,
                    0,
                    &[],
                    &tcp_segment((sport, 7000), n, 0, SYN, &[], &[]),
                ),
            ),
            Seed::SynWithOddOptions => ip(ME, 6, &tcp_segment((sport, 8080), n, 0, SYN, opts, &[])),
            Seed::SegmentToOwnConnection => {
                let seg = tcp_segment(
                    (80, self.port(s.b)),
                    n.rotate_left(13),
                    n,
                    n as u8,
                    opts,
                    &data,
                );
                ip(ME, 6, &seg)
            }
            Seed::SegmentToOtherCellsPort => {
                let seg = tcp_segment(
                    (80, self.port(s.a)),
                    n.rotate_left(13),
                    n,
                    n as u8,
                    &[],
                    &data,
                );
                ip(ME, 6, &seg)
            }
            Seed::SynAckOnCrafted => {
                let ports = (CRAFTED, crafted_port);
                let seq = their_seq.wrapping_add(1);
                ip(
                    ME,
                    6,
                    &tcp_segment(ports, CRAFTED_ISN, seq, SYN | ACK, opts, &[]),
                )
            }
            Seed::DataOnCrafted => {
                let ports = (CRAFTED, crafted_port);
                let seq = CRAFTED_ISN.wrapping_add(1 + (n >> 20));
                let flags = ACK | (n >> 24) as u8;
                let seg = tcp_segment(ports, seq, their_seq.wrapping_add(1), flags, &[], &data);
                ip(ME, 6, &seg)
            }
            Seed::UdpToOwnSocket => ip(
                ME,
                17,
                &udp(53, self.port(s.u), &bytes(n, (n % 1500) as usize)),
            ),
            Seed::UdpToClosedPort => ip(ME, 17, &udp(53, n as u16, &data)),
            Seed::UdpBroadcast => eth(
                ALL,
                0x0800,
                &ipv4(
                    EVIL,
                    [10, 0, 0, 255],
                    17,
                    0,
                    &[],
                    &udp(53, self.port(s.u), &data),
                ),
            ),
            Seed::Jumbo => ip(ME, 17, &udp(53, self.port(s.u), &bytes(n, 8000))),
            Seed::FirstFragment => {
                let mut dgram = udp(53, self.port(s.u), &bytes(n, 3000));
                dgram.truncate(1480);
                eth(NET_MAC, 0x0800, &ipv4(EVIL, ME, 17, 0x2000, &[], &dgram))
            }
            Seed::LaterFragment => eth(
                NET_MAC,
                0x0800,
                &ipv4(EVIL, ME, 17, 185, &[], &bytes(n, 1520)),
            ),
            Seed::IpOptions => {
                let mut opts = vec![7, 39, 4];
                opts.resize(40, 0);
                let dgram = udp(53, self.port(s.u), &data);
                eth(NET_MAC, 0x0800, &ipv4(EVIL, ME, 17, 0, &opts, &dgram))
            }
            Seed::IcmpEcho => {
                let echo = [
                    &[8, 0, 0, 0, 0, 1, 0, 1][..],
                    &bytes(n, (n % 1600) as usize),
                ]
                .concat();
                ip(ME, 1, &echo)
            }
            Seed::IcmpUnreachable => {
                let quoted = ipv4(
                    ME,
                    PEER,
                    6,
                    0,
                    &[],
                    &tcp_segment((self.port(s.a), 80), n, 0, ACK, &[], &[]),
                );
                let icmp = [&[3, (n % 16) as u8, 0, 0, 0, 0, 0, 0][..], &quoted[..28]].concat();
                ip(ME, 1, &icmp)
            }
            Seed::ArpRequest => eth(ALL, 0x0806, &arp(1, EVIL, [0; 6], ME)),
            Seed::ArpReply => {
                let sender = [10, 0, 0, (n % 254 + 1) as u8];
                eth(NET_MAC, 0x0806, &arp(2, sender, NET_MAC, ME))
            }
            Seed::DhcpOffer => dhcp(2, n.to_be_bytes(), ME, n),
            Seed::OtherEthertype => eth(NET_MAC, [0x86dd, 0x8100, 0x88cc][n as usize % 3], &data),
        };
        fix_checksums(&mut f);
        f
    }

    /// Hands `f` to the card, unless it claims to be the peer.
    fn inject(&mut self, f: Vec<u8>) {
        if !claims_the_peer(&f) {
            self.link.inbox.push_back(f);
        }
    }

    fn play(&mut self, step: &Step, s: &Sockets) {
        match step {
            Step::Run(ms) => self.run(u64::from(*ms), 1, s),
            Step::Wait(secs) => self.run(u64::from(*secs) * 1000, 50, s),
            Step::Talk => self.talk(s),
            Step::Seed(seed, n, m) => {
                let mut f = self.seed(*seed, *n, s);
                m.apply(&mut f);
                self.inject(f);
            }
            Step::Replay(i, m) => {
                if !self.captured.is_empty() {
                    let mut f = self.captured[usize::from(*i) % self.captured.len()].clone();
                    m.apply(&mut f);
                    self.inject(f);
                }
            }
            Step::Raw(bytes, ip) => {
                let mut f = bytes.clone();
                if *ip && f.len() >= 14 {
                    f[..14].copy_from_slice(&eth(NET_MAC, 0x0800, &[]));
                    fix_checksums(&mut f);
                }
                self.inject(f);
            }
            Step::Flood(units) => {
                for i in 0..u32::from(*units) * 8 {
                    let seed = if i % 4 == 3 {
                        Seed::UdpToOwnSocket
                    } else {
                        Seed::SynToListener
                    };
                    let f = self.seed(seed, i, s);
                    self.inject(f);
                }
            }
        }
    }

    /// Closes every socket, lets 75 s pass (past every timeout and the
    /// linger time) and empties every queue.
    fn close(&mut self, s: &Sockets) {
        for h in s.all() {
            self.net.release(h);
        }
        for _ in 0..150 {
            self.now += 500 * MS;
            self.exchange();
        }
        assert_eq!(self.net.open(), 0, "every socket closed");
        assert!(!self.net.wire_busy(), "every connection left smoltcp");
        for q in [&mut self.link, &mut self.peer.link, &mut self.evil.link] {
            q.sent = Default::default();
            q.inbox = Default::default();
        }
        self.captured = Vec::new();
        self.crafted = None;
        self.sent = Vec::new();
        self.got = Vec::new();
    }
}

/// Reads what is waiting on loopback socket `h`.
fn loopback(net: &mut Net, h: Handle) -> Vec<u8> {
    let mut out = Vec::new();
    let _ = net.recv(h, usize::MAX, false, &mut |b| {
        out.extend_from_slice(b);
        true
    });
    out
}

fn hostile_frames(steps: &[Step]) {
    let mut w = World::new();
    // Once through, so every buffer that keeps its room has it.
    let s = w.open();
    w.talk(&s);
    w.run(50, 1, &s);
    w.close(&s);
    let baseline = live();

    let s = w.open();
    let a_local = w.net.local_addr(s.a).unwrap();
    for step in steps {
        w.play(step, &s);
        assert_eq!(w.net.open(), 8, "no socket opened or lost");
    }
    // Cell A's connection still works: everything it sent comes back.
    w.talk(&s);
    for _ in 0..1000 {
        if w.got.len() == w.sent.len() {
            break;
        }
        w.run(10, 10, &s);
    }
    assert_eq!(
        String::from_utf8_lossy(&w.got),
        String::from_utf8_lossy(&w.sent)
    );
    assert!(!w.sent.is_empty());
    assert_eq!(w.net.poll(s.a) & (events::ERR | events::HUP), 0);
    assert_eq!(w.net.local_addr(s.a).unwrap(), a_local);
    assert_eq!(w.net.peer_addr(s.a), Ok(Addr::Inet { ip: PEER, port: 80 }));
    assert_eq!(loopback(&mut w.net, s.ls), b"LOOPBACK-SECRET-1");
    assert_eq!(loopback(&mut w.net, s.lc), b"LOOPBACK-SECRET-2");
    w.net.end_call(|| None);
    w.close(&s);
    assert_eq!(live(), baseline, "heap back to where it was");
}

/// DHCP: the client's transaction, read from what the card sent.
fn xid(sent: &std::collections::VecDeque<Vec<u8>>, last: [u8; 4]) -> [u8; 4] {
    sent.iter()
        .rev()
        .find(|f| f.len() >= 50 && f[12..14] == [8, 0] && f[23] == 17 && f[36..38] == [0, 67])
        .map_or(last, |f| f[46..50].try_into().unwrap())
}

#[derive(Debug, Clone)]
enum DhcpStep {
    /// This many tens of milliseconds pass.
    Run(u16),
    /// A reply of `kind` (offer 2, ACK 5, NAK 6, or any) offering `yiaddr`
    /// for `lease` seconds, to the client's last transaction.
    Reply {
        kind: u8,
        yiaddr: [u8; 4],
        lease: u32,
        m: Mutation,
    },
    Raw(Vec<u8>),
}

fn dhcp_step() -> impl Strategy<Value = DhcpStep> {
    let kind = prop_oneof![Just(2u8), Just(5), Just(6), any::<u8>()];
    let yiaddr = prop_oneof![
        3 => (2u8..255).prop_map(|h| [10, 0, 0, h]),
        1 => any::<[u8; 4]>(),
    ];
    let lease = prop_oneof![Just(0u32), Just(u32::MAX), 1u32..100_000, any::<u32>()];
    prop_oneof![
        2 => (0u16..2000).prop_map(DhcpStep::Run),
        5 => (kind, yiaddr, lease, mutation())
            .prop_map(|(kind, yiaddr, lease, m)| DhcpStep::Reply { kind, yiaddr, lease, m }),
        1 => prop::collection::vec(any::<u8>(), 0..600).prop_map(DhcpStep::Raw),
    ]
}

/// Plays `steps` against a card asking for a lease; returns its address.
fn hostile_dhcp(steps: &[DhcpStep]) -> Option<[u8; 4]> {
    let mut link = QueueLink {
        mac: NET_MAC,
        ..QueueLink::default()
    };
    let mut net = Net::new();
    net.attach(&mut link, WireConfig::Dhcp, 0, 3);
    let (mut now, mut last) = (MS, [0; 4]);
    net.poll_wire(&mut link, now);
    last = xid(&link.sent, last);
    link.sent = Default::default();
    let baseline = live();
    for step in steps {
        match step {
            DhcpStep::Run(tens) => {
                for _ in 0..*tens {
                    now += 10 * MS;
                    net.poll_wire(&mut link, now);
                    last = xid(&link.sent, last);
                    link.sent.clear();
                }
            }
            DhcpStep::Reply {
                kind,
                yiaddr,
                lease,
                m,
            } => {
                let mut f = dhcp(*kind, last, *yiaddr, *lease);
                fix_checksums(&mut f);
                m.apply(&mut f);
                link.inbox.push_back(f);
            }
            DhcpStep::Raw(b) => link.inbox.push_back(b.clone()),
        }
        now += MS;
        net.poll_wire(&mut link, now);
        last = xid(&link.sent, last);
        link.sent.clear();
        if let Some(ip) = net.wire_addr() {
            let unicast = ip != [0; 4] && ip != [255; 4] && !(224..240).contains(&ip[0]);
            assert!(unicast, "{ip:?}");
        }
    }
    link.sent = Default::default();
    link.inbox = Default::default();
    assert_eq!(live(), baseline, "heap back to where it was");
    net.wire_addr()
}

proptest! {
    #[test]
    fn frames_from_the_wire_cannot_hurt_the_kernel(
        steps in prop::collection::vec(step(), 1..40),
    ) {
        hostile_frames(&steps);
    }

    #[test]
    fn dhcp_replies_cannot_hurt_the_kernel(
        steps in prop::collection::vec(dhcp_step(), 1..30),
    ) {
        hostile_dhcp(&steps);
    }
}

#[test]
fn a_valid_lease_configures_the_card() {
    let reply = |kind| DhcpStep::Reply {
        kind,
        yiaddr: [10, 0, 0, 9],
        lease: 3600,
        m: Mutation {
            edits: Vec::new(),
            cut: None,
            fix: false,
        },
    };
    let steps = [reply(2), DhcpStep::Run(10), reply(5)];
    assert_eq!(hostile_dhcp(&steps), Some([10, 0, 0, 9]));
}

/// The seeds reach what they aim at, so their changed copies test those
/// paths and not only the first checks.
#[test]
fn seeds_reach_what_they_aim_at() {
    let mut w = World::new();
    let s = w.open();
    let mut answer = |seed, n| {
        let f = w.seed(seed, n, &s);
        w.inject(f);
        w.now += MS;
        w.net.poll_wire(&mut w.link, w.now);
        let sent: Vec<_> = w.link.sent.iter().cloned().collect();
        w.route();
        sent
    };
    let rst = answer(Seed::SynToListener, 1);
    assert!(
        rst.iter()
            .any(|f| tcp_of(f).is_some_and(|t| t.3 & 0x04 != 0)),
        "a reset"
    );
    let icmp = answer(Seed::UdpToClosedPort, 9);
    assert!(
        icmp.iter()
            .any(|f| f.len() > 34 && f[23] == 1 && f[34] == 3),
        "unreachable"
    );
    let arp = answer(Seed::ArpRequest, 0);
    assert!(
        arp.iter()
            .any(|f| f[12..14] == [8, 6] && f[20..22] == [0, 2]),
        "an ARP reply"
    );
    answer(Seed::UdpToOwnSocket, 5);
    answer(Seed::SynAckOnCrafted, 0);
    answer(Seed::DataOnCrafted, 50);
    let len = |net: &mut Net, h| net.recv(h, 100, false, &mut |_| true).map(|r| r.len);
    assert_eq!(len(&mut w.net, s.u), Ok(5), "a datagram");
    let crafted = Addr::Inet {
        ip: EVIL,
        port: CRAFTED,
    };
    assert_eq!(w.net.connect(s.c, &crafted), Ok(()), "a handshake");
    assert_eq!(len(&mut w.net, s.c), Ok(50), "data in order");
}
