//! The network card (design D2 and D5 of `add-virtio-net`): `smoltcp` on a
//! [`NetLink`] for every address outside the loopback network.
//!
//! A TCP connection to the outside is a `smoltcp` socket in state
//! [`State::Wire`]; a UDP socket that sent to the outside also has a
//! `smoltcp` socket on its port. [`Net::poll_wire`] moves frames between the
//! card and `smoltcp` and counts the changes waiting cells wait for.

use super::*;
use alloc::vec;
use rhizo_hal::NetLink;
use smoltcp::iface::{Config as IfaceConfig, Interface, PollResult, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::{dhcpv4, tcp, udp};
use smoltcp::time::{Duration, Instant};
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, IpEndpoint, Ipv4Address};

/// Bytes of a TCP connection's buffers to and from the wire, each way.
const TCP_BUF: usize = 64 << 10;
/// Datagrams and bytes a UDP socket buffers to and from the wire.
const UDP_PACKETS: usize = 16;
const UDP_BYTES: usize = 64 << 10;
/// A connection that gets no answer gives up after this long.
const TIMEOUT: Duration = Duration::from_secs(20);
/// A closed connection whose peer does not close is aborted after this
/// long (as Linux's FIN_WAIT_2 timeout does).
const LINGER: Duration = Duration::from_secs(60);

/// How the card gets its address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireConfig {
    /// DHCPv4.
    Dhcp,
    /// A static address, its prefix length and the gateway.
    Static {
        addr: [u8; 4],
        prefix: u8,
        gateway: Option<[u8; 4]>,
    },
}

/// `smoltcp` on the card.
pub(crate) struct Wire {
    iface: Interface,
    sockets: SocketSet<'static>,
    dhcp: Option<SocketHandle>,
    addr: Option<[u8; 4]>,
    /// Connections closed by the cells, dropped from `smoltcp` once done,
    /// or aborted at the time given (a peer that never closes its side).
    closing: Vec<(SocketHandle, Option<Instant>)>,
}

impl core::fmt::Debug for Wire {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Wire")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

/// The card as `smoltcp` sees it.
pub(crate) struct Phy<'a> {
    link: &'a mut dyn NetLink,
    frame: Vec<u8>,
}

impl<'a> Phy<'a> {
    pub(crate) fn new(link: &'a mut dyn NetLink) -> Self {
        Self {
            link,
            frame: Vec::new(),
        }
    }
}

pub(crate) struct Rx<'a>(&'a [u8]);
pub(crate) struct Tx<'a>(&'a mut dyn NetLink);

impl RxToken for Rx<'_> {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(self.0)
    }
}

impl TxToken for Tx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut frame = vec![0u8; len];
        let r = f(&mut frame);
        self.0.send(&frame);
        r
    }
}

impl Device for Phy<'_> {
    type RxToken<'b>
        = Rx<'b>
    where
        Self: 'b;
    type TxToken<'b>
        = Tx<'b>
    where
        Self: 'b;

    fn receive(&mut self, _: Instant) -> Option<(Rx<'_>, Tx<'_>)> {
        let frame = &mut self.frame;
        frame.clear();
        if !self.link.recv(&mut |f| frame.extend_from_slice(f)) {
            return None;
        }
        Some((Rx(&self.frame), Tx(&mut *self.link)))
    }

    fn transmit(&mut self, _: Instant) -> Option<Tx<'_>> {
        Some(Tx(&mut *self.link))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1514;
        caps
    }
}

fn instant(now_ns: u64) -> Instant {
    Instant::from_micros((now_ns / 1000) as i64)
}

fn v4(ip: [u8; 4]) -> Ipv4Address {
    Ipv4Address::new(ip[0], ip[1], ip[2], ip[3])
}

fn endpoint(e: IpEndpoint) -> Addr {
    let IpAddress::Ipv4(ip) = e.addr;
    Addr::Inet {
        ip: ip.octets(),
        port: e.port,
    }
}

/// What a TCP connection to the outside is, as its socket's calls see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Progress {
    Connecting,
    /// Established or past it (closing in some direction).
    Open,
    /// Closed without the cell closing it: refused, reset or timed out.
    Failed,
}

impl Wire {
    pub(crate) fn tcp(&mut self, handle: SocketHandle) -> &mut tcp::Socket<'static> {
        self.sockets.get_mut::<tcp::Socket>(handle)
    }

    fn set_address(&mut self, cidr: Option<([u8; 4], u8)>, gateway: Option<[u8; 4]>) {
        self.iface.update_ip_addrs(|addrs| {
            addrs.clear();
            if let Some((ip, prefix)) = cidr {
                let _ = addrs.push(IpCidr::new(IpAddress::Ipv4(v4(ip)), prefix));
            }
        });
        self.iface.routes_mut().remove_default_ipv4_route();
        if let Some(gw) = gateway {
            let _ = self.iface.routes_mut().add_default_ipv4_route(v4(gw));
        }
        self.addr = cidr.map(|(ip, _)| ip);
    }
}

impl Net {
    /// Puts the card behind `link` on the network, with an address as
    /// `config` says. `seed` makes `smoltcp`'s sequence numbers differ
    /// between boots (the caller's randomness, never read here).
    pub fn attach(&mut self, link: &mut dyn NetLink, config: WireConfig, now_ns: u64, seed: u64) {
        let mut iface_config =
            IfaceConfig::new(HardwareAddress::Ethernet(EthernetAddress(link.mac())));
        iface_config.random_seed = seed;
        let mut phy = Phy::new(link);
        let iface = Interface::new(iface_config, &mut phy, instant(now_ns));
        let mut wire = Wire {
            iface,
            sockets: SocketSet::new(Vec::new()),
            dhcp: None,
            addr: None,
            closing: Vec::new(),
        };
        match config {
            WireConfig::Dhcp => wire.dhcp = Some(wire.sockets.add(dhcpv4::Socket::new())),
            WireConfig::Static {
                addr,
                prefix,
                gateway,
            } => wire.set_address(Some((addr, prefix)), gateway),
        }
        self.wire = Some(wire);
    }

    /// The card's address, once it has one.
    pub fn wire_addr(&self) -> Option<[u8; 4]> {
        self.wire.as_ref().and_then(|w| w.addr)
    }

    /// Whether cells have sockets on the wire (the card should then be
    /// polled often, design D4).
    pub fn wire_busy(&self) -> bool {
        self.wire
            .as_ref()
            .is_some_and(|w| w.sockets.iter().count() > usize::from(w.dhcp.is_some()))
    }

    /// Moves frames between the card and `smoltcp`, runs its timers and
    /// counts what changed. Returns when it wants to run again
    /// (nanoseconds from now), if it knows.
    pub fn poll_wire(&mut self, link: &mut dyn NetLink, now_ns: u64) -> Option<u64> {
        let wire = self.wire.as_mut()?;
        let now = instant(now_ns);
        let mut phy = Phy::new(link);
        if wire.iface.poll(now, &mut phy, &mut wire.sockets) == PollResult::SocketStateChanged {
            self.changes += 1;
            // Connections that opened, so a later close is a reset, not a
            // refusal.
            for s in self.sockets.iter_mut().flatten() {
                if let State::Wire {
                    handle,
                    ever_open: ever_open @ false,
                    ..
                } = &mut s.state
                {
                    let state = wire.sockets.get::<tcp::Socket>(*handle).state();
                    *ever_open = !matches!(
                        state,
                        tcp::State::SynSent | tcp::State::SynReceived | tcp::State::Closed
                    );
                }
            }
        }
        if let Some(h) = wire.dhcp {
            let lease = match wire.sockets.get_mut::<dhcpv4::Socket>(h).poll() {
                Some(dhcpv4::Event::Configured(c)) => Some(Some((
                    c.address.address().octets(),
                    c.address.prefix_len(),
                    c.router,
                ))),
                Some(dhcpv4::Event::Deconfigured) => Some(None),
                None => None,
            };
            match lease {
                Some(Some((ip, prefix, router))) => {
                    wire.set_address(Some((ip, prefix)), router.map(|r| r.octets()));
                    self.changes += 1;
                }
                Some(None) => {
                    wire.set_address(None, None);
                    self.changes += 1;
                }
                None => {}
            }
        }
        let Wire {
            sockets, closing, ..
        } = wire;
        closing.retain_mut(|(h, give_up)| {
            let give_up = *give_up.get_or_insert(now + LINGER);
            let socket = sockets.get_mut::<tcp::Socket>(*h);
            if now >= give_up {
                socket.abort();
            }
            let done = matches!(socket.state(), tcp::State::Closed | tcp::State::TimeWait);
            if done {
                sockets.remove(*h);
            }
            !done
        });
        wire.iface
            .poll_delay(now, &wire.sockets)
            .map(|d| d.total_micros() * 1000)
    }

    /// Turns Nagle's algorithm off (`no_delay`) or on for wire socket `handle`.
    pub(crate) fn wire_no_delay(&mut self, handle: SocketHandle, no_delay: bool) {
        if let Some(wire) = self.wire.as_mut() {
            wire.sockets
                .get_mut::<tcp::Socket>(handle)
                .set_nagle_enabled(!no_delay);
        }
    }

    /// Starts a TCP connection from `h` to `ip:port` outside: `InProgress`.
    pub(crate) fn wire_connect(&mut self, h: Handle, ip: [u8; 4], port: u16) -> Result<()> {
        let Some(mine) = self.wire_addr() else {
            return Err(Error::NetUnreach);
        };
        let local = match self.get(h)?.local {
            Some(Addr::Inet { port, .. }) => port,
            _ => self.ephemeral(h, Type::Stream)?,
        };
        let wire = self.wire.as_mut().ok_or(Error::NetUnreach)?;
        let mut socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0u8; TCP_BUF]),
            tcp::SocketBuffer::new(vec![0u8; TCP_BUF]),
        );
        socket.set_timeout(Some(TIMEOUT));
        let opts = self.sockets[h as usize]
            .as_ref()
            .map(|s| s.opts)
            .unwrap_or_default();
        socket.set_nagle_enabled(!opts.no_delay);
        if socket
            .connect(wire.iface.context(), (v4(ip), port), (v4(mine), local))
            .is_err()
        {
            return Err(Error::Invalid);
        }
        let handle = wire.sockets.add(socket);
        let s = self.get_mut(h)?;
        s.local = Some(Addr::Inet {
            ip: mine,
            port: local,
        });
        s.remote = Some(Addr::Inet { ip, port });
        s.state = State::Wire {
            handle,
            ever_open: false,
            report: true,
        };
        s.refused = false;
        self.changes += 1;
        Err(Error::InProgress)
    }

    /// Where TCP connection `handle` stands.
    pub(crate) fn wire_progress(&self, handle: SocketHandle) -> Progress {
        let Some(wire) = self.wire.as_ref() else {
            return Progress::Failed;
        };
        match wire.sockets.get::<tcp::Socket>(handle).state() {
            tcp::State::SynSent | tcp::State::SynReceived => Progress::Connecting,
            tcp::State::Closed | tcp::State::Listen => Progress::Failed,
            _ => Progress::Open,
        }
    }

    /// Sends what fits of `data` on connection `handle`.
    pub(crate) fn wire_send(&mut self, handle: SocketHandle, data: &[u8]) -> Result<usize> {
        let wire = self.wire.as_mut().ok_or(Error::Pipe)?;
        let s = wire.tcp(handle);
        if !s.may_send() {
            return Err(Error::Pipe);
        }
        if !s.can_send() {
            return Err(Error::Again);
        }
        s.send_slice(data).map_err(|_| Error::Pipe)
    }

    /// Hands `take` up to `len` received bytes of connection `handle`.
    pub(crate) fn wire_recv(
        &mut self,
        handle: SocketHandle,
        len: usize,
        peek: bool,
        take: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Result<usize> {
        let wire = self.wire.as_mut().ok_or(Error::NotConn)?;
        let s = wire.tcp(handle);
        if !s.can_recv() {
            return if s.may_recv() {
                Err(Error::Again)
            } else {
                Ok(0)
            };
        }
        if peek {
            let data = s.peek(len).map_err(|_| Error::NotConn)?;
            return Ok(if take(data) { data.len() } else { 0 });
        }
        // The buffer is a ring: its bytes come in at most two pieces.
        let mut done = 0;
        for _ in 0..2 {
            if done == len || !s.can_recv() {
                break;
            }
            let got = s
                .recv(|buf| {
                    let n = buf.len().min(len - done);
                    let n = if take(&buf[..n]) { n } else { 0 };
                    (n, n)
                })
                .map_err(|_| Error::NotConn)?;
            if got == 0 {
                break;
            }
            done += got;
        }
        Ok(done)
    }

    /// The `poll` events of connection `handle`.
    pub(crate) fn wire_poll(&self, handle: SocketHandle) -> u32 {
        let Some(wire) = self.wire.as_ref() else {
            return ERR | HUP;
        };
        let s = wire.sockets.get::<tcp::Socket>(handle);
        let mut ev = 0;
        if s.can_recv() || !s.may_recv() {
            ev |= IN;
        }
        if !s.may_recv() {
            ev |= RDHUP;
        }
        if s.can_send() {
            ev |= OUT;
        }
        if !s.may_recv() && !s.may_send() {
            ev |= HUP;
        }
        ev
    }

    /// Sends a FIN on connection `handle` (`shutdown` for writing).
    pub(crate) fn wire_shutdown(&mut self, handle: SocketHandle) {
        if let Some(wire) = self.wire.as_mut() {
            wire.tcp(handle).close();
            self.changes += 1;
        }
    }

    /// The cells are done with connection `handle`: it closes, and leaves
    /// `smoltcp` once closed.
    pub(crate) fn wire_close(&mut self, handle: SocketHandle) {
        if let Some(wire) = self.wire.as_mut() {
            wire.tcp(handle).close();
            wire.closing.push((handle, None));
        }
    }

    /// Sends a datagram from `h` (bound to `port`) to `ip:dport` outside;
    /// dropped if `smoltcp` has no room, as UDP would.
    pub(crate) fn wire_send_to(
        &mut self,
        h: Handle,
        port: u16,
        data: &[u8],
        ip: [u8; 4],
        dport: u16,
    ) -> Result<()> {
        let handle = self.wire_udp(h, port)?;
        let wire = self.wire.as_mut().ok_or(Error::NetUnreach)?;
        let _ = wire
            .sockets
            .get_mut::<udp::Socket>(handle)
            .send_slice(data, (v4(ip), dport));
        Ok(())
    }

    /// The `smoltcp` UDP socket of `h` on `port`, made the first time.
    fn wire_udp(&mut self, h: Handle, port: u16) -> Result<SocketHandle> {
        if self.wire_addr().is_none() {
            return Err(Error::NetUnreach);
        }
        if let Some(handle) = self.get(h)?.udp {
            return Ok(handle);
        }
        let wire = self.wire.as_mut().ok_or(Error::NetUnreach)?;
        let buffer = || {
            udp::PacketBuffer::new(
                vec![udp::PacketMetadata::EMPTY; UDP_PACKETS],
                vec![0u8; UDP_BYTES],
            )
        };
        let mut socket = udp::Socket::new(buffer(), buffer());
        socket.bind(port).map_err(|_| Error::AddrInUse)?;
        let handle = wire.sockets.add(socket);
        self.get_mut(h)?.udp = Some(handle);
        Ok(handle)
    }

    /// The next datagram from the wire for UDP socket `handle`.
    pub(crate) fn wire_recv_from(
        &mut self,
        handle: SocketHandle,
        len: usize,
        peek: bool,
        take: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Option<Received> {
        let wire = self.wire.as_mut()?;
        let s = wire.sockets.get_mut::<udp::Socket>(handle);
        let (data, meta) = if peek {
            let (data, meta) = s.peek().ok()?;
            (data, *meta)
        } else {
            s.recv().ok()?
        };
        let n = len.min(data.len());
        if !take(&data[..n]) {
            return Some(Received {
                len: 0,
                full: 0,
                from: None,
            });
        }
        Some(Received {
            len: n,
            full: data.len(),
            from: Some(endpoint(meta.endpoint)),
        })
    }

    /// Whether UDP socket `handle` has a datagram from the wire.
    pub(crate) fn wire_udp_ready(&self, handle: SocketHandle) -> bool {
        self.wire
            .as_ref()
            .is_some_and(|w| w.sockets.get::<udp::Socket>(handle).can_recv())
    }

    /// Drops UDP socket `handle` from `smoltcp`.
    pub(crate) fn wire_drop(&mut self, handle: SocketHandle) {
        if let Some(wire) = self.wire.as_mut() {
            wire.sockets.remove(handle);
        }
    }
}
