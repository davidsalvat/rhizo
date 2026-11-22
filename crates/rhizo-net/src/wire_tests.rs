//! Host tests of the network card path: a [`Net`] on a [`QueueLink`]
//! talks to a `smoltcp` peer (10.0.0.1) over an in-memory wire.

use super::tests::woken;
use super::wire::Phy;
use super::*;
use rhizo_hal::testing::QueueLink;
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::socket::{tcp, udp};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr};

pub(crate) const ME: [u8; 4] = [10, 0, 0, 2];
pub(crate) const PEER: [u8; 4] = [10, 0, 0, 1];
pub(crate) const MS: u64 = 1_000_000;

/// The other end of the wire: echoes TCP on port 80 and UDP on port 53.
pub(crate) struct Peer {
    iface: Interface,
    sockets: SocketSet<'static>,
    pub(crate) link: QueueLink,
    tcp: SocketHandle,
    udp: SocketHandle,
    /// Whether it closes its side once the client closed its own.
    closes: bool,
    /// How long it thinks before echoing (nanoseconds), and what it holds
    /// meanwhile with the time it arrived.
    delay: u64,
    held: Option<(u64, Vec<u8>)>,
    /// Whether it closes the connection after each echo (a server ending
    /// a kept-alive connection between requests).
    hangs_up: bool,
}

/// A socket listening on port 80, for the peer's next client.
fn listener() -> tcp::Socket<'static> {
    let mut tcp = tcp::Socket::new(
        tcp::SocketBuffer::new(vec![0; 4096]),
        tcp::SocketBuffer::new(vec![0; 4096]),
    );
    tcp.listen(80).unwrap();
    tcp
}

impl Peer {
    fn new() -> Self {
        Self::at([2, 0, 0, 0, 0, 1], PEER)
    }

    /// A peer with MAC address `mac` and address `ip`/24.
    pub(crate) fn at(mac: [u8; 6], ip: [u8; 4]) -> Self {
        let mut link = QueueLink {
            mac,
            ..QueueLink::default()
        };
        let config = Config::new(HardwareAddress::Ethernet(EthernetAddress(link.mac)));
        let mut phy = Phy::new(&mut link);
        let mut iface = Interface::new(config, &mut phy, Instant::from_micros(0));
        iface.update_ip_addrs(|a| {
            let [a0, a1, a2, a3] = ip;
            a.push(IpCidr::new(IpAddress::v4(a0, a1, a2, a3), 24))
                .unwrap();
        });
        let mut sockets = SocketSet::new(Vec::new());
        let mut udp = udp::Socket::new(
            udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 4], vec![0; 4096]),
            udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 4], vec![0; 4096]),
        );
        udp.bind(53).unwrap();
        Self {
            tcp: sockets.add(listener()),
            udp: sockets.add(udp),
            iface,
            sockets,
            link,
            closes: true,
            delay: 0,
            held: None,
            hangs_up: false,
        }
    }

    pub(crate) fn step(&mut self, now: u64) {
        let mut phy = Phy::new(&mut self.link);
        self.iface.poll(
            Instant::from_micros((now / 1000) as i64),
            &mut phy,
            &mut self.sockets,
        );
        let t = self.sockets.get_mut::<tcp::Socket>(self.tcp);
        if t.can_recv() {
            let mut buf = [0u8; 512];
            let n = t.recv_slice(&mut buf).unwrap();
            let (_, held) = self.held.get_or_insert((now, Vec::new()));
            held.extend_from_slice(&buf[..n]);
        }
        if let Some((since, _)) = self.held
            && now >= since + self.delay
            && let Some((_, data)) = self.held.take()
        {
            t.send_slice(&data).unwrap();
            if self.hangs_up {
                // It finishes closing on its own while a new socket
                // listens for the next client, as a server's would.
                t.close();
                self.tcp = self.sockets.add(listener());
                return;
            }
        }
        if self.closes && t.state() == tcp::State::CloseWait {
            t.close(); // the client is done: so is the echo
        }
        if matches!(t.state(), tcp::State::Closed | tcp::State::TimeWait) {
            // Ready for the next client at once.
            self.held = None;
            self.sockets.remove(self.tcp);
            self.tcp = self.sockets.add(listener());
        }
        let u = self.sockets.get_mut::<udp::Socket>(self.udp);
        if let Ok((data, meta)) = u.recv() {
            let (data, from) = (data.to_vec(), meta.endpoint);
            u.send_slice(&data, from).unwrap();
        }
    }
}

/// A kernel's sockets on a card with address 10.0.0.2/24, and the peer.
struct World {
    net: Net,
    link: QueueLink,
    peer: Peer,
    now: u64,
}

impl World {
    fn new() -> Self {
        let mut link = QueueLink {
            mac: [2, 0, 0, 0, 0, 2],
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
            peer: Peer::new(),
            now: 0,
        }
    }

    /// Lets frames cross the wire for `ms` milliseconds.
    fn run(&mut self, ms: u64) {
        for _ in 0..ms {
            self.now += MS;
            self.net.poll_wire(&mut self.link, self.now);
            self.peer.link.inbox.extend(self.link.sent.drain(..));
            self.peer.step(self.now);
            self.link.inbox.extend(self.peer.link.sent.drain(..));
            self.net.poll_wire(&mut self.link, self.now);
        }
    }

    fn connect(&mut self, port: u16) -> Handle {
        let s = self.net.socket(Family::Inet, Type::Stream).unwrap();
        let to = Addr::Inet { ip: PEER, port };
        assert_eq!(self.net.connect(s, &to), Err(Error::InProgress));
        assert_eq!(
            self.net.connect(s, &to),
            Err(Error::Again),
            "still connecting"
        );
        assert_eq!(self.net.poll(s), 0);
        self.run(20);
        s
    }
}

fn drain(net: &mut Net, h: Handle) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    net.recv(h, usize::MAX, false, &mut |b| {
        out.extend_from_slice(b);
        true
    })?;
    Ok(out)
}

#[test]
fn tcp_to_a_host_outside() {
    let mut w = World::new();
    let before = w.net.changes();
    let s = w.connect(80);
    assert!(w.net.changes() > before, "the handshake wakes waiters");
    let to = Addr::Inet { ip: PEER, port: 80 };
    assert_eq!(
        w.net.connect(s, &to),
        Ok(()),
        "a connect made again reports it"
    );
    assert_eq!(w.net.connect(s, &to), Err(Error::IsConn));
    assert_eq!(w.net.poll(s) & OUT, OUT);
    assert_eq!(w.net.peer_addr(s), Ok(to));
    assert!(matches!(
        w.net.local_addr(s),
        Ok(Some(Addr::Inet { ip: ME, port })) if EPHEMERAL.contains(&port)
    ));
    assert_eq!(drain(&mut w.net, s), Err(Error::Again));
    assert_eq!(w.net.send(s, b"hello, outside", None), Ok(14));
    w.run(20);
    assert_eq!(w.net.poll(s) & IN, IN);
    assert_eq!(drain(&mut w.net, s).unwrap(), b"hello, outside");
    w.net.shutdown(s, Shutdown::Write).unwrap();
    assert_eq!(w.net.send(s, b"x", None), Err(Error::Pipe));
    w.net.release(s);
    w.run(50);
    assert!(!w.net.wire_busy(), "closed connections leave smoltcp");
}

/// Frames the card sends for two 1-byte writes on a connection, the
/// second before the peer could acknowledge the first, with `TCP_NODELAY`
/// set after connecting or not.
fn frames_for_two_writes(no_delay: bool) -> usize {
    let mut w = World::new();
    let s = w.connect(80);
    let opts = Options {
        no_delay,
        ..w.net.options(s).unwrap()
    };
    w.net.set_options(s, opts).unwrap();
    w.link.sent.clear();
    for byte in [b"a", b"b"] {
        assert_eq!(w.net.send(s, byte, None), Ok(1));
        w.now += MS;
        w.net.poll_wire(&mut w.link, w.now);
    }
    w.link.sent.len()
}

#[test]
fn no_delay_set_on_a_connection_applies_at_once() {
    assert_eq!(
        frames_for_two_writes(false),
        1,
        "Nagle holds the second byte"
    );
    assert_eq!(
        frames_for_two_writes(true),
        2,
        "TCP_NODELAY sends it at once"
    );
}

#[test]
fn a_peer_that_never_closes_is_given_up() {
    let mut w = World::new();
    let s = w.connect(80);
    w.peer.closes = false;
    w.net.release(s);
    w.run(20);
    assert!(w.net.wire_busy(), "waiting for the peer's FIN");
    w.run(61_000);
    assert!(!w.net.wire_busy(), "aborted after the linger time");
}

#[test]
fn a_port_nobody_listens_on_refuses() {
    let mut w = World::new();
    let s = w.connect(81);
    assert_eq!(w.net.poll(s) & ERR, ERR);
    let to = Addr::Inet { ip: PEER, port: 81 };
    assert_eq!(w.net.connect(s, &to), Err(Error::ConnRefused));
    let t = w.connect(81);
    assert_eq!(
        w.net.take_error(t),
        Ok(Some(Error::ConnRefused)),
        "SO_ERROR"
    );
}

#[test]
fn without_an_address_the_outside_is_unreachable() {
    let mut net = Net::new();
    let s = net.socket(Family::Inet, Type::Stream).unwrap();
    let to = Addr::Inet { ip: PEER, port: 80 };
    assert_eq!(net.connect(s, &to), Err(Error::NetUnreach));
    let mut link = QueueLink::default();
    net.attach(&mut link, WireConfig::Dhcp, 0, 1);
    assert_eq!(net.connect(s, &to), Err(Error::NetUnreach), "no lease yet");
    net.poll_wire(&mut link, MS);
    let discover = link.sent.pop_front().expect("a DHCP discover");
    assert_eq!(&discover[12..14], &[0x08, 0x00], "IPv4");
    assert_eq!(
        &discover[36..38],
        &67u16.to_be_bytes(),
        "to the DHCP server port"
    );
}

#[test]
fn udp_to_a_host_outside() {
    let mut w = World::new();
    let u = w.net.socket(Family::Inet, Type::Dgram).unwrap();
    let to = Addr::Inet { ip: PEER, port: 53 };
    assert_eq!(w.net.send(u, b"query", Some(&to)), Ok(5));
    assert_eq!(w.net.poll(u) & IN, 0);
    w.run(20);
    assert_eq!(w.net.poll(u) & IN, IN);
    let mut got = Vec::new();
    let r = w
        .net
        .recv(u, 100, false, &mut |d| {
            got.extend_from_slice(d);
            true
        })
        .unwrap();
    assert_eq!((got.as_slice(), r.from), (&b"query"[..], Some(to)));
    assert_eq!(w.net.recv(u, 100, false, &mut |_| true), Err(Error::Again));
}

/// A call that reaches the card, a TCP connection or a datagram out, says
/// so in the caller's ports; loopback calls, and calls without a card, do
/// not (design D22 of `add-cell-hierarchy`).
#[test]
fn calls_that_reach_the_card_mark_the_caller() {
    let mut w = World::new();
    let call = |net: &mut Net, f: &mut dyn FnMut(&mut Net)| {
        net.enter(Ports::new(3));
        f(net);
        net.leave().unwrap().out
    };
    let local = Addr::Inet {
        ip: LOOPBACK,
        port: 9,
    };
    let s = w.net.socket(Family::Inet, Type::Stream).unwrap();
    let u = w.net.socket(Family::Inet, Type::Dgram).unwrap();
    assert!(!call(&mut w.net, &mut |n| {
        let _ = n.connect(s, &local);
        let _ = n.send(u, b"here", Some(&local));
    }));
    let tcp = Addr::Inet { ip: PEER, port: 80 };
    assert!(call(&mut w.net, &mut |n| {
        let _ = n.connect(s, &tcp);
    }));
    let dns = Addr::Inet { ip: PEER, port: 53 };
    assert!(call(&mut w.net, &mut |n| {
        let _ = n.send(u, b"query", Some(&dns));
    }));
    let mut cardless = Net::new();
    let s = cardless.socket(Family::Inet, Type::Stream).unwrap();
    assert!(!call(&mut cardless, &mut |n| {
        let _ = n.connect(s, &tcp);
    }));
}

#[test]
fn binding_takes_the_card_s_address_only() {
    let mut w = World::new();
    let s = w.net.socket(Family::Inet, Type::Stream).unwrap();
    assert_eq!(w.net.bind(s, &Addr::Inet { ip: ME, port: 9 }), Ok(()));
    let t = w.net.socket(Family::Inet, Type::Stream).unwrap();
    assert_eq!(
        w.net.bind(
            t,
            &Addr::Inet {
                ip: [10, 0, 0, 3],
                port: 9
            }
        ),
        Err(Error::AddrNotAvail)
    );
}

#[test]
fn wire_connections_reset_on_fork() {
    let mut w = World::new();
    let s = w.connect(80);
    let child = w.net.fork(s, 3).unwrap();
    assert_eq!(drain(&mut w.net, child), Err(Error::ConnReset));
    assert_eq!(drain(&mut w.net, child).unwrap(), b"");
    assert_eq!(w.net.send(child, b"x", None), Err(Error::Pipe));
    assert_eq!(w.net.send(s, b"still", None), Ok(5), "the parent keeps it");
    w.run(20);
    assert_eq!(drain(&mut w.net, s).unwrap(), b"still");
}

#[test]
fn wire_connections_stay_up_while_parked() {
    let mut w = World::new();
    let s = w.connect(80);
    assert_eq!(w.net.send(s, b"lost once", None), Ok(9));
    // The segment leaves, and the wire loses it.
    w.net.poll_wire(&mut w.link, w.now);
    assert!(!w.link.sent.is_empty());
    w.link.sent.clear();
    let r = w.net.export(s).unwrap();
    assert!(r.parked.is_some());
    w.net.release(s);
    assert!(w.net.wire_busy(), "the card is still polled for it");
    // Suspended for 3 s: `smoltcp` retransmits, the echo comes back and
    // is acknowledged into the parked socket's buffer.
    w.run(3000);
    let peer = w.peer.sockets.get::<tcp::Socket>(w.peer.tcp);
    assert_eq!(peer.state(), tcp::State::Established);
    assert_eq!(peer.send_queue(), 0, "the peer's echo was acknowledged");
    let (s2, _) = w.net.import(&r, 0).unwrap();
    assert_eq!(s2, s);
    assert_eq!(drain(&mut w.net, s).unwrap(), b"lost once");
    assert_eq!(w.net.send(s, b"after", None), Ok(5));
    w.run(20);
    assert_eq!(drain(&mut w.net, s).unwrap(), b"after");
}

#[test]
fn a_reset_while_parked_is_reported_after() {
    let mut w = World::new();
    let s = w.connect(80);
    let r = w.net.export(s).unwrap();
    w.net.release(s);
    let peer = w.peer.tcp;
    w.peer.sockets.get_mut::<tcp::Socket>(peer).abort();
    w.run(20);
    let (s, _) = w.net.import(&r, 0).unwrap();
    assert_eq!(drain(&mut w.net, s), Err(Error::ConnReset));
    assert_eq!(drain(&mut w.net, s).unwrap(), b"");
}

#[test]
fn a_reset_from_outside_is_reported_once() {
    let mut w = World::new();
    let s = w.connect(80);
    let peer = w.peer.tcp;
    w.peer.sockets.get_mut::<tcp::Socket>(peer).abort();
    w.run(20);
    assert_eq!(w.net.poll(s) & ERR, ERR);
    assert_eq!(drain(&mut w.net, s), Err(Error::ConnReset));
    assert_eq!(drain(&mut w.net, s).unwrap(), b"");
}

#[test]
fn frames_from_the_wire_wake_the_waiters_of_its_sockets() {
    let mut w = World::new();
    let s = w.connect(80);
    let loopback = w.net.socket(Family::Inet, Type::Stream).unwrap();
    w.net.end_call(|| None);
    assert_eq!(drain(&mut w.net, s), Err(Error::Again));
    w.net.end_call(|| Some(1));
    w.net.poll(loopback);
    w.net.end_call(|| Some(2));
    w.net.send(s, b"echo", None).unwrap();
    w.net.end_call(|| None);
    woken(&mut w.net);
    w.run(20);
    let woken = woken(&mut w.net);
    assert!(woken.contains(&1), "the echo arrived");
    assert!(!woken.contains(&2), "nothing for a loopback socket");
}

/// A model that thinks for 45 s before it answers: its request is
/// acknowledged at once, then nothing crosses the wire for longer than the
/// card's timeout (20 s), and the answer still arrives.
#[test]
fn a_connection_silent_for_long_still_gets_its_reply() {
    let mut w = World::new();
    let s = w.connect(80);
    w.peer.delay = 45_000 * MS;
    assert_eq!(w.net.send(s, b"question", None), Ok(8));
    w.run(45_000);
    assert_eq!(drain(&mut w.net, s), Err(Error::Again), "still thinking");
    assert_eq!(w.net.poll(s) & (ERR | HUP), 0, "and still connected");
    w.run(20);
    assert_eq!(drain(&mut w.net, s).unwrap(), b"question");
    // Idle for ten minutes, then used again.
    w.peer.delay = 0;
    w.run(600_000);
    assert_eq!(w.net.send(s, b"again", None), Ok(5));
    w.run(20);
    assert_eq!(drain(&mut w.net, s).unwrap(), b"again");
}

/// Bytes a peer never acknowledges are still given up after the timeout.
#[test]
fn bytes_a_peer_never_acknowledges_are_given_up() {
    let mut w = World::new();
    let s = w.connect(80);
    assert_eq!(w.net.send(s, b"lost", None), Ok(4));
    // The peer is gone: nothing reaches it any more.
    for _ in 0..19_000 {
        w.now += MS;
        w.net.poll_wire(&mut w.link, w.now);
        w.link.sent.clear();
    }
    assert_eq!(w.net.poll(s) & ERR, 0, "retransmitting");
    for _ in 0..2_000 {
        w.now += MS;
        w.net.poll_wire(&mut w.link, w.now);
        w.link.sent.clear();
    }
    assert_eq!(drain(&mut w.net, s), Err(Error::ConnReset));
}

/// A new connection to the peer, open within `ms` milliseconds.
fn reconnect(w: &mut World, ms: u64) -> Handle {
    let s = w.net.socket(Family::Inet, Type::Stream).unwrap();
    let to = Addr::Inet { ip: PEER, port: 80 };
    assert_eq!(w.net.connect(s, &to), Err(Error::InProgress));
    w.run(ms);
    assert_eq!(w.net.connect(s, &to), Ok(()), "open within {ms} ms");
    s
}

#[test]
fn a_connection_reset_by_the_peer_is_replaced_at_once() {
    let mut w = World::new();
    let s = w.connect(80);
    let peer = w.peer.tcp;
    w.peer.sockets.get_mut::<tcp::Socket>(peer).abort();
    w.run(5);
    assert_eq!(drain(&mut w.net, s), Err(Error::ConnReset));
    w.net.release(s);
    let s = reconnect(&mut w, 5);
    assert_eq!(w.net.send(s, b"after the reset", None), Ok(15));
    w.run(5);
    assert_eq!(drain(&mut w.net, s).unwrap(), b"after the reset");
}

#[test]
fn a_connection_the_peer_ends_between_requests_is_replaced_at_once() {
    let mut w = World::new();
    w.peer.hangs_up = true;
    let s = w.connect(80);
    assert_eq!(w.net.send(s, b"turn 1", None), Ok(6));
    w.run(20);
    assert_eq!(drain(&mut w.net, s).unwrap(), b"turn 1");
    assert_eq!(drain(&mut w.net, s).unwrap(), b"", "then the peer's end");
    w.net.release(s);
    let s = reconnect(&mut w, 5);
    assert_eq!(w.net.send(s, b"turn 2", None), Ok(6));
    w.run(5);
    assert_eq!(drain(&mut w.net, s).unwrap(), b"turn 2");
}

/// A card under a flood from the network: it always has one more frame.
struct Flood {
    taken: usize,
}

impl rhizo_hal::NetLink for Flood {
    fn mac(&self) -> [u8; 6] {
        [2, 0, 0, 0, 0, 2]
    }
    fn send(&mut self, _: &[u8]) -> bool {
        true
    }
    fn recv(&mut self, take: &mut dyn FnMut(&[u8])) -> bool {
        self.taken += 1;
        assert!(
            self.taken < 100_000,
            "the flood held the kernel in poll_wire"
        );
        take(&[0xff; 60]);
        true
    }
}

#[test]
fn a_flood_from_the_wire_lets_the_kernel_go() {
    let mut net = Net::new();
    let mut link = Flood { taken: 0 };
    let config = WireConfig::Static {
        addr: ME,
        prefix: 24,
        gateway: None,
    };
    net.attach(&mut link, config, 0, 7);
    assert_eq!(net.poll_wire(&mut link, MS), Some(0), "poll again soon");
    assert_eq!(link.taken, wire::POLL_FRAMES, "one ring of frames per poll");
}
