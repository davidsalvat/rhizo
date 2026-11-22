//! The sockets of a kernel's cells (spec `networking`, design D1, D2, D4 and
//! D6 of `add-sockets-and-poll`).
//!
//! [`Net`] holds every socket of every cell; a cell's descriptors hold
//! [`Handle`]s, counted once per file description. On the loopback network
//! (127.0.0.0/8) a TCP connection is a pair of byte queues joined here and a
//! UDP datagram goes straight into the queue of the socket bound at its
//! destination; a Unix stream socket is reached by its name number, which a
//! file of type socket in the cell's tree holds.
//!
//! **Networks** (design D5 of `add-cell-hierarchy`). Every socket belongs to
//! one loopback network, numbered by the kernel: each cell has its own, the
//! number its calls enter with ([`Ports::key`], the cell's identifier), and
//! network 0 is the kernel's own. Bind conflicts, the listener a `connect`
//! reaches, UDP delivery and Unix names are all looked up in the socket's
//! network only, so no cell reaches, or sees the ports of, another's. A
//! copy's sockets are bound again in the copy's network ([`Net::fork`]), a
//! resumed cell's come back in its new one ([`Net::import`]).
//!
//! **Egress** (design D6 and D7 of `add-cell-hierarchy`). A cell's sockets
//! reach outside the loopback network only where its network's egress
//! policy allows ([`Net::set_egress`]; `None`, the default, allows
//! nothing), and never where the host's deny floor forbids
//! ([`Net::set_floor`]; by default the card's gateway and name servers,
//! the host as the guest sees it). Connections and datagrams out are
//! checked, datagrams in are dropped unless their source is allowed, and
//! narrowing a policy resets the connections it no longer allows. A socket
//! through the card keeps its network's port for its cell, while the port
//! on the card is drawn at random by the kernel and shown to no cell. The
//! kernel's own network 0 is not a cell's: it reaches everything. Every other address goes
//! through the network card, with `smoltcp` (module `wire`). Nothing here
//! reads a clock or touches hardware: time is passed in, the card is a
//! [`rhizo_hal::NetLink`], and blocking is the caller's business
//! ([`Error::Again`]): each socket keeps a wait queue of the waiters that
//! looked at it before they blocked, and a change to a socket hands its
//! waiters to the caller ([`Net::end_call`], [`Net::drain_woken`]; design D1
//! of `add-socket-wait-queues`).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod endpoints;
mod record;
mod wire;

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::cell::RefCell;

pub use record::{DecodeError, Park, Record, Saved, decode_records, encode_records};
use rhizo_cap::{Channels, Rights};
pub use rhizo_cap::{Egress, Proto, Rule};
use smoltcp::iface::SocketHandle;
pub use wire::WireConfig;
use wire::{Progress, Wire};

/// A socket, as a descriptor holds it.
pub type Handle = u32;

/// Bytes a stream socket may hold unread.
pub const STREAM_BUF: usize = 256 << 10;
/// Bytes of datagrams a socket may hold unread.
pub const DGRAM_BUF: usize = 256 << 10;
/// Largest UDP payload.
pub const DGRAM_MAX: usize = 65_507;
/// Most sockets open at once in the kernel.
pub const SOCKETS_MAX: usize = 4096;
/// Kernel memory one socket may take, counted against the memory of the
/// cell holding it from when it is made (design D4 and D23 of
/// `add-cell-hierarchy`, never per packet): its record and its queue
/// filled, which is at least what `smoltcp` buffers for a connection
/// through the card both ways.
pub const SOCKET_BYTES: u64 = (STREAM_BUF + size_of::<Socket>()) as u64;
const _: () = assert!(STREAM_BUF >= 2 * wire::TCP_BUF && DGRAM_BUF <= STREAM_BUF);
/// Largest accepted `listen` backlog.
pub const BACKLOG_MAX: usize = 4096;
/// Longest Unix socket path (`sun_path` without its NUL).
pub const UNIX_PATH_MAX: usize = 107;
/// Ports handed out by `bind` to port 0 and by implicit binds.
pub const EPHEMERAL: core::ops::RangeInclusive<u16> = 32768..=60999;
/// `INADDR_ANY`.
pub const ANY: [u8; 4] = [0; 4];
/// `INADDR_LOOPBACK`.
pub const LOOPBACK: [u8; 4] = [127, 0, 0, 1];

/// `poll` event bits (the same values as `EPOLL*`).
pub mod events {
    pub const IN: u32 = 0x1;
    pub const OUT: u32 = 0x4;
    pub const ERR: u32 = 0x8;
    pub const HUP: u32 = 0x10;
    pub const RDHUP: u32 = 0x2000;
}
use events::*;

/// Address family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Inet,
    Unix,
}

/// Socket type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    Stream,
    Dgram,
    /// A Unix sequenced-packet socket: unconnected, or a channel endpoint
    /// (design D10 of `add-cell-hierarchy`, module `endpoints`).
    SeqPacket,
}

/// A socket address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Addr {
    /// IPv4 address and port.
    Inet { ip: [u8; 4], port: u16 },
    /// A named Unix socket: its name number (held by the file of type
    /// socket) and the path it was bound with.
    Unix { name: u64, path: Vec<u8> },
    /// A Unix socket without a name.
    UnixUnnamed,
}

/// Why a socket operation failed, with its Linux error number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// It would wait (`EAGAIN`).
    Again,
    AddrInUse,
    AddrNotAvail,
    NetUnreach,
    ConnRefused,
    ConnReset,
    NotConn,
    IsConn,
    /// Writing to a connection closed for writing (`EPIPE`).
    Pipe,
    Invalid,
    OpNotSupp,
    DestAddrReq,
    MsgSize,
    /// [`SOCKETS_MAX`] sockets are open (`ENFILE`).
    TooMany,
    /// A connection to the outside has started (`EINPROGRESS`).
    InProgress,
    /// A message with more handles than a channel carries
    /// (`ETOOMANYREFS`).
    TooManyRefs,
    /// The cell's egress policy or the host's deny floor forbids the
    /// destination (`EACCES`, as Landlock: "denied", not "no route").
    Access,
}

impl Error {
    /// The Linux `errno` value.
    pub const fn errno(self) -> i64 {
        match self {
            Error::Again => 11,
            Error::Access => 13,
            Error::TooMany => 23,
            Error::Pipe => 32,
            Error::Invalid => 22,
            Error::DestAddrReq => 89,
            Error::MsgSize => 90,
            Error::TooManyRefs => 109,
            Error::OpNotSupp => 95,
            Error::AddrInUse => 98,
            Error::AddrNotAvail => 99,
            Error::NetUnreach => 101,
            Error::ConnReset => 104,
            Error::IsConn => 106,
            Error::NotConn => 107,
            Error::ConnRefused => 111,
            Error::InProgress => 115,
        }
    }
}

/// Result of a socket operation.
pub type Result<T> = core::result::Result<T, Error>;

/// Socket options, as `getsockopt` reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub reuse_addr: bool,
    pub reuse_port: bool,
    pub keepalive: bool,
    pub no_delay: bool,
    pub broadcast: bool,
    /// `SO_LINGER`: seconds, when on.
    pub linger: Option<u32>,
    pub rcvbuf: u32,
    pub sndbuf: u32,
    /// `SO_RCVTIMEO` in nanoseconds (0: none).
    pub rcv_timeout_ns: u64,
    /// `SO_SNDTIMEO` in nanoseconds (0: none).
    pub snd_timeout_ns: u64,
    pub keep_idle: u32,
    pub keep_intvl: u32,
    pub keep_cnt: u32,
}

impl Default for Options {
    /// Linux's defaults.
    fn default() -> Self {
        Self {
            reuse_addr: false,
            reuse_port: false,
            keepalive: false,
            no_delay: false,
            broadcast: false,
            linger: None,
            rcvbuf: 131_072,
            sndbuf: 16_384,
            rcv_timeout_ns: 0,
            snd_timeout_ns: 0,
            keep_idle: 7200,
            keep_intvl: 75,
            keep_cnt: 9,
        }
    }
}

/// Which directions `shutdown` closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shutdown {
    Read,
    Write,
    Both,
}

/// What a receive handed over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Received {
    /// Bytes handed to the caller.
    pub len: usize,
    /// Length of the whole datagram (stream: `len`).
    pub full: usize,
    /// Sender of a datagram.
    pub from: Option<Addr>,
}

#[derive(Debug, Clone)]
enum State {
    /// Not connected or listening (every datagram socket).
    Open,
    Listening {
        backlog: usize,
        /// Connected server ends not accepted yet (no references).
        queue: VecDeque<Handle>,
    },
    /// A stream connection; `peer` is gone once the peer closed.
    Connected { peer: Option<Handle> },
    /// Reset: the next call reports `ECONNRESET` (`ECONNREFUSED` if it
    /// never opened) while `pending`; later ones see a closed connection.
    Reset { pending: bool },
    /// A TCP connection through the network card; `report` until a
    /// `connect` made again has reported it open.
    Wire {
        handle: SocketHandle,
        ever_open: bool,
        report: bool,
    },
    /// A channel endpoint as one grant holds it (design D10 of
    /// `add-cell-hierarchy`): its endpoint in [`Channels`], the grant and
    /// its rights, and the next socket on the same endpoint.
    Endpoint {
        end: u32,
        grant: u32,
        rights: Rights,
        next: Handle,
    },
}

#[derive(Debug, Clone)]
struct Socket {
    refs: u32,
    /// The loopback network it belongs to (0: the kernel's).
    net: u32,
    family: Family,
    ty: Type,
    local: Option<Addr>,
    /// Peer of a connection, or default destination of a datagram socket.
    remote: Option<Addr>,
    opts: Options,
    state: State,
    rx: VecDeque<u8>,
    /// No more stream bytes will arrive (peer shut down writing or closed).
    eof: bool,
    shut_rd: bool,
    shut_wr: bool,
    dgrams: VecDeque<(Addr, Vec<u8>)>,
    dgram_bytes: usize,
    /// The `smoltcp` socket of a UDP socket that sent to the wire.
    udp: Option<SocketHandle>,
    /// Its port on the network card, drawn at random and shown to no cell
    /// (0: none).
    card: u16,
    /// A reset connection was refused (it never opened).
    refused: bool,
    /// Its wait queue: the waiters that looked at it before they blocked,
    /// woken by its next change.
    waiters: Vec<u32>,
}

impl Socket {
    fn new(family: Family, ty: Type, opts: Options, net: u32) -> Self {
        Self {
            refs: 1,
            net,
            family,
            ty,
            local: None,
            remote: None,
            opts,
            state: State::Open,
            rx: VecDeque::new(),
            eof: false,
            shut_rd: false,
            shut_wr: false,
            dgrams: VecDeque::new(),
            dgram_bytes: 0,
            udp: None,
            card: 0,
            refused: false,
            waiters: Vec::new(),
        }
    }

    fn port(&self) -> Option<([u8; 4], u16)> {
        match self.local {
            Some(Addr::Inet { ip, port }) => Some((ip, port)),
            _ => None,
        }
    }
}

/// A cell's loopback network as its calls enter it: the network's number
/// and where its ephemeral ports go next (design D2 of
/// `close-cross-cell-leaks`, D5 of `add-cell-hierarchy`). Each network
/// walks the range in order from a start of its own, which the kernel's
/// secret and `key` choose, skipping only its own ports in use, so the
/// ports a cell gets do not show what other cells took: the kernel keeps
/// it with the cell and hands it to [`Net::enter`] for the cell's calls.
/// Not part of a snapshot: a resumed cell has a new identifier, hence a new
/// network and start, as a new boot has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ports {
    /// The cell's identifier, which numbers its network (never 0, the
    /// kernel's).
    pub key: u32,
    /// The next port to try; 0 before the first.
    pub next: u16,
    /// The call reached the network card (set here, never cleared): the
    /// cell may now hold a secret (design D22 of `add-cell-hierarchy`).
    pub out: bool,
    /// The last destination the call was refused by the cell's egress
    /// policy or the deny floor (`EACCES`), for the audit log (design D7
    /// and D18 of `add-cell-hierarchy`); `None` if none was.
    pub denied: Option<(Proto, [u8; 4], u16)>,
    /// The cell has a memory limit, so its sockets were reserved for its
    /// domain (design D4 of `add-cell-hierarchy`): it may take sockets that
    /// [`Net::set_reserved`] keeps from the others.
    pub reserved: bool,
}

impl Ports {
    /// The ports of the cell with identifier `key`, none taken yet.
    pub const fn new(key: u32) -> Self {
        Self {
            key,
            next: 0,
            out: false,
            denied: None,
            reserved: false,
        }
    }
}

/// Every socket of the kernel.
#[derive(Debug)]
pub struct Net {
    sockets: Vec<Option<Socket>>,
    open: usize,
    changes: u64,
    /// The next ephemeral port of sockets the kernel opens for itself
    /// (calls made with no [`Ports`] entered).
    next_port: u16,
    /// The network and ephemeral ports of the cell whose call holds the
    /// sockets.
    caller: Option<Ports>,
    next_name: u64,
    /// The network card, once attached.
    wire: Option<Wire>,
    /// Identity of this boot, in parked records (design D2 of
    /// `keep-tcp-across-suspend`).
    boot: u64,
    next_key: u64,
    /// The secret that turns `next_key` into a parked record's key (finding
    /// S15 of `docs/security.md`): a forged snapshot cannot name another
    /// cell's parked connection by guessing it.
    secret: [u64; 2],
    /// Listeners and connections of suspended cells, by key, each holding
    /// one reference.
    parked: Vec<(u64, Handle)>,
    /// Waiters taken off the wait queues of sockets that changed, for the
    /// caller to wake ([`drain_woken`](Self::drain_woken)).
    woken: Vec<u32>,
    /// The egress policies of the networks that have one but `None`.
    policies: Vec<(u32, Egress)>,
    /// The host's deny floor beyond the card's gateway and name servers,
    /// and the destinations carved out of it.
    floor: Vec<Rule>,
    carved: Vec<Rule>,
    /// Card ports drawn so far.
    card_draws: u64,
    /// Sockets kept for the cells of reserved domains, which no other
    /// cell's call may take ([`set_reserved`](Self::set_reserved)).
    reserved: usize,
    /// The sockets the current call looked at: its waiter joins their wait
    /// queues if it blocks ([`end_call`](Self::end_call)).
    looked: RefCell<Vec<Handle>>,
    /// The kernel's channels, whose endpoints sockets hold.
    channels: Channels,
    /// Per channel endpoint, the first socket holding it (`NO_SOCKET`:
    /// none), the others linked through [`State::Endpoint`].
    ends: Vec<Handle>,
    /// Grants whose last socket closed, for the kernel to drop
    /// ([`take_dropped`](Self::take_dropped)).
    dropped: Vec<u32>,
    /// Sockets in flight on endpoints that closed, still to release, and
    /// whether a release is draining them (module `endpoints`).
    orphans: Vec<Handle>,
    draining: bool,
    /// Each cell's inbox, a listening sequenced-packet socket the kernel
    /// holds one reference to (module `endpoints`).
    inboxes: Vec<(u32, Handle)>,
    /// Channel endpoints the kernel holds for a cell's observers and input
    /// (module `endpoints`).
    held: Vec<Held>,
    /// Cells with input waiting in an endpoint the kernel holds.
    inputs: Vec<u32>,
    /// The creators of cells' channels that ended, one entry per channel:
    /// their reserved room comes back to them.
    refunds: Vec<u32>,
    /// The kernel has something to do after the call ([`Net::take_after`]).
    after: bool,
    /// The calling cell received from another cell ([`Net::take_received`]).
    received: bool,
}

/// A channel endpoint the kernel holds for cell `cell` (no socket holds
/// it), through the cell handle `grant` that opened it: it sends the
/// cell's output to the peer when `observe`, and takes what the peer sends
/// as the cell's standard input when `input`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Held {
    end: u32,
    cell: u32,
    grant: u32,
    observe: bool,
    input: bool,
}

impl Default for Net {
    fn default() -> Self {
        Self::new()
    }
}

/// For a reset socket, whether its reset is still to be reported (and it
/// is reported now); `None` for any other state.
fn reset_pending(state: &mut State) -> Option<bool> {
    match state {
        State::Reset { pending } => Some(core::mem::replace(pending, false)),
        _ => None,
    }
}

/// Spreads the bits of `x` over the whole word (the SplitMix64 finalizer):
/// nearby boot identities pick far-apart ports.
fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn is_loopback(ip: [u8; 4]) -> bool {
    ip[0] == 127
}

impl Net {
    /// No sockets.
    pub const fn new() -> Self {
        Self {
            sockets: Vec::new(),
            open: 0,
            changes: 0,
            next_port: *EPHEMERAL.start(),
            caller: None,
            next_name: 1,
            wire: None,
            boot: 0,
            next_key: 1,
            secret: [0; 2],
            parked: Vec::new(),
            woken: Vec::new(),
            looked: RefCell::new(Vec::new()),
            policies: Vec::new(),
            floor: Vec::new(),
            carved: Vec::new(),
            card_draws: 0,
            reserved: 0,
            channels: Channels::new(),
            ends: Vec::new(),
            dropped: Vec::new(),
            orphans: Vec::new(),
            draining: false,
            inboxes: Vec::new(),
            held: Vec::new(),
            inputs: Vec::new(),
            refunds: Vec::new(),
            after: false,
            received: false,
        }
    }

    /// Keeps `n` sockets for the cells of reserved domains (design D4 of
    /// `add-cell-hierarchy`: twice their roots' socket limits, a connection
    /// costing two): a call of a cell without a memory limit is refused a
    /// socket that would leave fewer free. The kernel's own calls and those
    /// of reserved cells are not.
    pub fn set_reserved(&mut self, n: usize) {
        self.reserved = n;
    }

    /// The sockets [`set_reserved`](Self::set_reserved) keeps.
    pub fn reserved(&self) -> usize {
        self.reserved
    }

    /// Sets the egress policy of network `net` (design D7 of
    /// `add-cell-hierarchy`); `Egress::None` is every network's until set.
    /// Its connections through the card that the policy does not allow are
    /// reset (each reports `ECONNRESET` at its next call), and datagrams
    /// waiting from sources it does not allow are dropped.
    pub fn set_egress(&mut self, net: u32, egress: Egress) {
        self.policies.retain(|(n, _)| *n != net);
        if egress != Egress::None {
            self.policies.push((net, egress));
        }
        self.enforce(Some(net));
    }

    /// Network `net` is gone (its cell ended, or its snapshot was dropped):
    /// its policy goes.
    pub fn drop_network(&mut self, net: u32) {
        self.policies.retain(|(n, _)| *n != net);
    }

    /// The cell of network `old` was resumed as `new`: its policy follows
    /// (a suspended cell's network keeps it meanwhile).
    pub fn rename_network(&mut self, old: u32, new: u32) {
        if let Some((n, _)) = self.policies.iter_mut().find(|(n, _)| *n == old) {
            *n = new;
        }
    }

    /// Whether a network has a policy other than `None`.
    pub fn has_policies(&self) -> bool {
        !self.policies.is_empty()
    }

    /// Sets the host's deny floor (design D7): `deny` holds at most
    /// [`FLOOR_MAX`](rhizo_cap::limits::FLOOR_MAX) rules no policy reaches,
    /// beyond the card's gateway and name servers (always in the floor);
    /// `carved` the exact destinations allowed through it all the same (the
    /// model endpoints). Connections the floor now forbids are reset.
    pub fn set_floor(&mut self, deny: &[Rule], carved: &[Rule]) -> Result<()> {
        let max = rhizo_cap::limits::FLOOR_MAX;
        if deny.len() > max || carved.len() > max {
            return Err(Error::Invalid);
        }
        self.floor = deny.to_vec();
        self.carved = carved.to_vec();
        self.enforce(None);
        Ok(())
    }

    /// Whether a socket of network `net` may reach `ip`:`port` outside with
    /// `proto` (TCP or UDP), or receive a datagram from it: never through
    /// the floor (unless carved out of it), then as the network's policy
    /// says. The kernel's network 0 reaches everything.
    pub fn allows(&self, net: u32, proto: Proto, ip: [u8; 4], port: u16) -> bool {
        if net == 0 {
            return true;
        }
        let Some((_, egress)) = self.policies.iter().find(|(n, _)| *n == net) else {
            return false;
        };
        let host = self.wire.as_ref().is_some_and(|w| w.is_host(ip));
        if (host || self.floor.iter().any(|r| r.allows(proto, ip, port)))
            && !self.carved.iter().any(|r| r.allows(proto, ip, port))
        {
            return false;
        }
        egress.allows(proto, ip, port)
    }

    /// Resets the connections through the card of network `net` (every
    /// network: `None`) that their policy no longer allows, and drops the
    /// datagrams waiting from sources it does not allow.
    fn enforce(&mut self, net: Option<u32>) {
        for h in 0..self.sockets.len() as Handle {
            let Some(s) = self.sockets[h as usize].as_ref() else {
                continue;
            };
            if net.is_some_and(|n| n != s.net) {
                continue;
            }
            if let (&State::Wire { handle, .. }, Some(Addr::Inet { ip, port })) =
                (&s.state, s.remote.clone())
                && !self.allows(s.net, Proto::Tcp, ip, port)
            {
                self.wire_abort(handle);
                if let Ok(s) = self.get_mut(h) {
                    s.state = State::Reset { pending: true };
                    s.refused = false;
                    s.eof = true;
                }
                self.touch(h);
            }
            self.drop_denied(h);
        }
    }

    /// Sets the identity of this boot (the caller's randomness, never read
    /// here): sockets parked in another boot do not re-attach, and ephemeral
    /// ports start at a place it chooses. A peer keeps the addresses and
    /// ports of a connection it closed for a while (`TIME_WAIT`); starting
    /// every boot at the first port would make the first connections of a
    /// new boot look like old ones to it (RFC 6056; design D1 of
    /// `fix-ephemeral-ports`).
    pub fn set_boot(&mut self, boot: u64) {
        self.boot = boot;
        let span = u64::from(EPHEMERAL.end() - EPHEMERAL.start()) + 1;
        self.next_port = EPHEMERAL.start() + (mix(boot) % span) as u16;
    }

    /// Sets the secret parked records' keys are derived from (the caller's
    /// randomness, never read here; design D1 of `fix-parked-keys`).
    pub fn set_secret(&mut self, secret: [u64; 2]) {
        self.secret = secret;
    }

    /// Starts a call of the cell whose network is `ports`: sockets it makes
    /// belong to that network, and ports it binds implicitly or to port 0
    /// come from it, until [`leave`](Self::leave).
    pub fn enter(&mut self, ports: Ports) {
        self.caller = Some(ports);
    }

    /// Ends the call [`enter`](Self::enter) started; returns its ports as
    /// the call left them.
    pub fn leave(&mut self) -> Option<Ports> {
        self.caller.take()
    }

    /// Notes that the calling cell's call was refused `proto` to `ip`:`port`
    /// by its egress policy or the floor; returns the error to report.
    fn deny(&mut self, proto: Proto, ip: [u8; 4], port: u16) -> Error {
        if let Some(caller) = &mut self.caller {
            caller.denied = Some((proto, ip, port));
        }
        Error::Access
    }

    /// Notes that the calling cell's call reaches the network card.
    fn went_out(&mut self) {
        if let Some(caller) = &mut self.caller {
            caller.out = true;
        }
    }

    /// Where the ephemeral ports of the cell with `key` start: anywhere in
    /// the range, as the kernel's secret and the boot choose for it, so no
    /// cell can tell another's.
    fn first_port(&self, key: u32) -> u16 {
        let span = u64::from(EPHEMERAL.end() - EPHEMERAL.start()) + 1;
        let at = siphash24(self.secret, u64::from(key)).wrapping_add(mix(self.boot));
        EPHEMERAL.start() + (at % span) as u16
    }

    /// Counts every change a waiter may wait for (data, connections,
    /// closes), for statistics.
    pub fn changes(&self) -> u64 {
        self.changes
    }

    /// A change a waiter of `h` may wait for: its wait queue is woken.
    fn touch(&mut self, h: Handle) {
        self.changes += 1;
        if let Some(Some(s)) = self.sockets.get_mut(h as usize) {
            self.woken.append(&mut s.waiters);
        }
    }

    /// The current call looked at `h`: it may wait for it.
    fn look(&self, h: Handle) {
        self.looked.borrow_mut().push(h);
    }

    /// The end of a call: if it blocks (`waiter` says who waits, asked only
    /// when the call looked at sockets), the waiter joins the wait queue of
    /// each socket the call looked at (once), and is woken by the next
    /// change to one of them. Returns whether waiters were woken since the
    /// last [`drain_woken`](Self::drain_woken), as
    /// [`has_woken`](Self::has_woken) does.
    pub fn end_call(&mut self, waiter: impl FnOnce() -> Option<u32>) -> bool {
        let looked = self.looked.get_mut();
        if !looked.is_empty() {
            if let Some(key) = waiter() {
                for &h in looked.iter() {
                    if let Some(Some(s)) = self.sockets.get_mut(h as usize)
                        && !s.waiters.contains(&key)
                    {
                        s.waiters.push(key);
                    }
                }
            }
            looked.clear();
        }
        !self.woken.is_empty()
    }

    /// Takes `waiter` off the wait queue of `h` (it is gone).
    pub fn forget(&mut self, h: Handle, waiter: u32) {
        if let Ok(s) = self.get_mut(h) {
            s.waiters.retain(|&k| k != waiter);
        }
    }

    /// Whether waiters were woken since the last
    /// [`drain_woken`](Self::drain_woken).
    pub fn has_woken(&self) -> bool {
        !self.woken.is_empty()
    }

    /// Takes waiter `key` off the waiters woken since the last
    /// [`drain_woken`](Self::drain_woken); returns whether it was there. A
    /// cell whose call woke its own waiting threads wakes them itself, in
    /// its slice.
    pub fn take_woken(&mut self, key: u32) -> bool {
        let before = self.woken.len();
        self.woken.retain(|&k| k != key);
        self.woken.len() != before
    }

    /// Hands `f` the waiters woken by changes since the last call, in the
    /// order they were woken (a waiter may come more than once); the list
    /// keeps its room, so a change allocates nothing.
    pub fn drain_woken(&mut self, f: impl FnMut(u32)) {
        self.woken.drain(..).for_each(f);
    }

    /// Sockets open.
    pub fn open(&self) -> usize {
        self.open
    }

    fn get(&self, h: Handle) -> Result<&Socket> {
        self.sockets
            .get(h as usize)
            .and_then(Option::as_ref)
            .ok_or(Error::Invalid)
    }

    fn get_mut(&mut self, h: Handle) -> Result<&mut Socket> {
        self.sockets
            .get_mut(h as usize)
            .and_then(Option::as_mut)
            .ok_or(Error::Invalid)
    }

    fn insert(&mut self, socket: Socket) -> Result<Handle> {
        let kept = match self.caller {
            Some(c) if !c.reserved => self.reserved,
            _ => 0,
        };
        if self.open.saturating_add(kept) >= SOCKETS_MAX {
            return Err(Error::TooMany);
        }
        self.open += 1;
        match self.sockets.iter().position(Option::is_none) {
            Some(i) => {
                self.sockets[i] = Some(socket);
                Ok(i as Handle)
            }
            None => {
                self.sockets.push(Some(socket));
                Ok((self.sockets.len() - 1) as Handle)
            }
        }
    }

    /// The network of the calling cell (0 when no cell entered).
    fn caller_net(&self) -> u32 {
        self.caller.map_or(0, |c| c.key)
    }

    /// A new socket with one reference, in the calling cell's network.
    /// Unix sockets are streams only.
    pub fn socket(&mut self, family: Family, ty: Type) -> Result<Handle> {
        if (family == Family::Unix && ty == Type::Dgram)
            || (family == Family::Inet && ty == Type::SeqPacket)
        {
            return Err(Error::OpNotSupp);
        }
        let net = self.caller_net();
        self.insert(Socket::new(family, ty, Options::default(), net))
    }

    /// Test builds: the card port of `h` (0: none), which the outside sees.
    #[cfg(test)]
    pub(crate) fn card_of(&self, h: Handle) -> u16 {
        self.get(h).map_or(0, |s| s.card)
    }

    /// The network `h` belongs to.
    pub fn network(&self, h: Handle) -> Result<u32> {
        Ok(self.get(h)?.net)
    }

    /// Two connected unnamed Unix stream sockets (`socketpair`).
    pub fn pair(&mut self) -> Result<(Handle, Handle)> {
        let a = self.socket(Family::Unix, Type::Stream)?;
        let b = match self.socket(Family::Unix, Type::Stream) {
            Ok(b) => b,
            Err(e) => {
                self.release(a);
                return Err(e);
            }
        };
        for (me, peer) in [(a, b), (b, a)] {
            let s = self.get_mut(me)?;
            s.local = Some(Addr::UnixUnnamed);
            s.remote = Some(Addr::UnixUnnamed);
            s.state = State::Connected { peer: Some(peer) };
        }
        Ok((a, b))
    }

    /// One more file description refers to `h`.
    pub fn retain(&mut self, h: Handle) {
        if let Ok(s) = self.get_mut(h) {
            s.refs += 1;
        }
    }

    /// One file description less refers to `h`; the last one closes it.
    pub fn release(&mut self, h: Handle) {
        let Ok(s) = self.get_mut(h) else { return };
        s.refs -= 1;
        if s.refs == 0 {
            self.close(h);
        }
    }

    /// Closes `h`: a connection's peer sees the end of the stream (or a
    /// reset if bytes were left unread), connections never accepted are
    /// reset.
    fn close(&mut self, h: Handle) {
        let Some(mut s) = self.sockets.get_mut(h as usize).and_then(Option::take) else {
            return;
        };
        self.open -= 1;
        self.changes += 1;
        self.woken.append(&mut s.waiters);
        if let Some(udp) = s.udp {
            self.wire_drop(udp);
        }
        match s.state {
            State::Wire { handle, .. } => self.wire_close(handle),
            State::Listening { queue, .. } => {
                for q in queue {
                    // An inbox's channels waiting there go as any closed
                    // endpoint socket does (module `endpoints`).
                    if let Ok(State::Endpoint { .. }) = self.get(q).map(|q| &q.state) {
                        self.orphans.push(q);
                        continue;
                    }
                    if let Ok(State::Connected { peer: Some(p) }) = self.get(q).map(|q| &q.state) {
                        let p = *p;
                        self.reset(p);
                    }
                    if self.sockets[q as usize].take().is_some() {
                        self.open -= 1;
                    }
                }
                self.drain_orphans();
            }
            State::Connected { peer: Some(p) } => {
                if !s.rx.is_empty() {
                    self.reset(p);
                } else if let Ok(peer) = self.get_mut(p) {
                    peer.eof = true;
                    peer.state = State::Connected { peer: None };
                    self.touch(p);
                }
            }
            State::Endpoint {
                end, grant, next, ..
            } => {
                self.dropped.push(grant);
                self.after = true;
                self.leave_end(h, end, next);
            }
            _ => {}
        }
    }

    /// Makes `h` a reset connection.
    fn reset(&mut self, h: Handle) {
        if let Ok(s) = self.get_mut(h) {
            s.state = State::Reset { pending: true };
            s.rx.clear();
            s.eof = true;
            self.touch(h);
        }
    }

    /// Family and type of `h`.
    pub fn kind(&self, h: Handle) -> Result<(Family, Type)> {
        let s = self.get(h)?;
        Ok((s.family, s.ty))
    }

    /// A new Unix name number, never handed out before by this kernel.
    pub fn new_name(&mut self) -> u64 {
        self.next_name += 1;
        self.next_name - 1
    }

    /// Whether another socket of `ty` in network `net` holds `ip:port` in a
    /// way `opts` cannot share. Other networks' ports never count.
    fn port_taken(
        &self,
        net: u32,
        me: Handle,
        ty: Type,
        ip: [u8; 4],
        port: u16,
        opts: &Options,
    ) -> bool {
        self.sockets.iter().enumerate().any(|(i, s)| {
            let Some(s) = s.as_ref().filter(|s| s.net == net) else {
                return false;
            };
            let Some((ip2, port2)) = s.port() else {
                return false;
            };
            let overlaps = port2 == port && (ip2 == ip || ip2 == ANY || ip == ANY);
            let listening = matches!(s.state, State::Listening { .. });
            let shared = (opts.reuse_port && s.opts.reuse_port)
                || (opts.reuse_addr && s.opts.reuse_addr && !listening);
            i != me as usize && s.ty == ty && s.family == Family::Inet && overlaps && !shared
        })
    }

    /// A free ephemeral port for `ty` in `me`'s network: the next one of
    /// the calling cell's [`Ports`], or of the kernel's own when no cell
    /// entered.
    fn ephemeral(&mut self, me: Handle, ty: Type) -> Result<u16> {
        let net = self.get(me)?.net;
        let span = (EPHEMERAL.end() - EPHEMERAL.start()) as u32 + 1;
        let mut next = match self.caller {
            Some(Ports { key, next: 0, .. }) => self.first_port(key),
            Some(Ports { next, .. }) => next,
            None => self.next_port,
        };
        let mut found = Err(Error::AddrInUse);
        for _ in 0..span {
            let port = next;
            next = if port == *EPHEMERAL.end() {
                *EPHEMERAL.start()
            } else {
                port + 1
            };
            if !self.port_taken(net, me, ty, ANY, port, &Options::default()) {
                found = Ok(port);
                break;
            }
        }
        match self.caller.as_mut() {
            Some(caller) => caller.next = next,
            None => self.next_port = next,
        }
        found
    }

    /// `bind`. An Inet address must be on the loopback network or
    /// `INADDR_ANY`; port 0 picks a free port.
    pub fn bind(&mut self, h: Handle, addr: &Addr) -> Result<()> {
        let s = self.get(h)?;
        if s.local.is_some() {
            return Err(Error::Invalid);
        }
        let (ty, opts, net) = (s.ty, s.opts, s.net);
        let local = match (s.family, addr) {
            (Family::Inet, &Addr::Inet { ip, port }) => {
                if ip != ANY && !is_loopback(ip) && Some(ip) != self.wire_addr() {
                    return Err(Error::AddrNotAvail);
                }
                let port = match port {
                    0 => self.ephemeral(h, ty)?,
                    p if self.port_taken(net, h, ty, ip, p, &opts) => {
                        return Err(Error::AddrInUse);
                    }
                    p => p,
                };
                Addr::Inet { ip, port }
            }
            (Family::Unix, Addr::Unix { name, path }) if *name != 0 => Addr::Unix {
                name: *name,
                path: path.clone(),
            },
            _ => return Err(Error::Invalid),
        };
        self.get_mut(h)?.local = Some(local);
        Ok(())
    }

    /// Binds an Inet socket that has no address to an ephemeral port.
    fn autobind(&mut self, h: Handle, ip: [u8; 4]) -> Result<()> {
        let s = self.get(h)?;
        if s.local.is_some() || s.family != Family::Inet {
            return Ok(());
        }
        let port = self.ephemeral(h, s.ty)?;
        self.get_mut(h)?.local = Some(Addr::Inet { ip, port });
        Ok(())
    }

    /// `listen`: an Inet stream socket without an address gets an
    /// ephemeral port. A listening socket may change its backlog.
    pub fn listen(&mut self, h: Handle, backlog: usize) -> Result<()> {
        let s = self.get(h)?;
        if s.ty != Type::Stream {
            return Err(Error::OpNotSupp);
        }
        match s.state {
            State::Open => {}
            State::Listening { .. } => {}
            _ => return Err(Error::Invalid),
        }
        if s.local.is_none() {
            if s.family == Family::Unix {
                return Err(Error::Invalid);
            }
            self.autobind(h, ANY)?;
        }
        let backlog = backlog.min(BACKLOG_MAX);
        let s = self.get_mut(h)?;
        match &mut s.state {
            State::Listening { backlog: b, .. } => *b = backlog,
            state => {
                *state = State::Listening {
                    backlog,
                    queue: VecDeque::new(),
                }
            }
        }
        Ok(())
    }

    /// Whether `h` listens.
    pub fn is_listening(&self, h: Handle) -> bool {
        matches!(self.get(h).map(|s| &s.state), Ok(State::Listening { .. }))
    }

    /// `accept`: the oldest connection waiting on listener `h`, with one
    /// reference.
    pub fn accept(&mut self, h: Handle) -> Result<Handle> {
        self.look(h);
        let s = self.get_mut(h)?;
        let inbox = s.ty == Type::SeqPacket;
        let State::Listening { queue, .. } = &mut s.state else {
            return Err(Error::Invalid);
        };
        let conn = queue.pop_front().ok_or(Error::Again)?;
        self.get_mut(conn)?.refs = 1;
        // A channel from an inbox is a handle another cell gave.
        if inbox {
            self.received();
        }
        Ok(conn)
    }

    /// The listener a stream connection to `addr` reaches in network
    /// `net`: a Unix name is keyed by (network, name), so a name number
    /// forged into a snapshot reaches nothing outside its own network.
    fn listener(&self, net: u32, family: Family, addr: &Addr) -> Result<Handle> {
        let found = self.sockets.iter().position(|s| {
            let Some(s) = s.as_ref().filter(|s| s.net == net) else {
                return false;
            };
            let listening = matches!(s.state, State::Listening { .. });
            listening
                && s.family == family
                && match (&s.local, addr) {
                    (Some(Addr::Inet { ip, port }), Addr::Inet { ip: to, port: p }) => {
                        port == p && (*ip == ANY || ip == to)
                    }
                    (Some(Addr::Unix { name, .. }), Addr::Unix { name: to, .. }) => name == to,
                    _ => false,
                }
        });
        found.map(|i| i as Handle).ok_or(Error::ConnRefused)
    }

    /// `connect`. A stream connection is established at once (queued on the
    /// listener); a datagram socket only records its default destination.
    pub fn connect(&mut self, h: Handle, addr: &Addr) -> Result<()> {
        self.look(h);
        let s = self.get(h)?;
        let (family, ty, net) = (s.family, s.ty, s.net);
        let addr = match (family, addr) {
            (Family::Inet, &Addr::Inet { ip, port }) => {
                let ip = if ip == ANY { LOOPBACK } else { ip };
                if !is_loopback(ip) {
                    return self.connect_out(h, ty, ip, port);
                }
                Addr::Inet { ip, port }
            }
            (Family::Unix, Addr::Unix { .. }) => addr.clone(),
            _ => return Err(Error::Invalid),
        };
        if ty == Type::Dgram {
            if let Addr::Inet { ip, .. } = addr {
                self.autobind(h, ip)?;
            }
            self.get_mut(h)?.remote = Some(addr);
            return Ok(());
        }
        match s.state {
            State::Open => {}
            State::Listening { .. } => return Err(Error::Invalid),
            _ => return Err(Error::IsConn),
        }
        let listener = self.listener(net, family, &addr)?;
        let l = self.get(listener)?;
        let State::Listening { backlog, queue } = &l.state else {
            return Err(Error::ConnRefused);
        };
        if queue.len() > *backlog {
            return Err(Error::ConnRefused);
        }
        let (server_local, opts) = (
            match (&l.local, &addr) {
                (Some(Addr::Inet { .. }), Addr::Inet { ip, port }) => Addr::Inet {
                    ip: *ip,
                    port: *port,
                },
                (local, _) => local.clone().unwrap_or(Addr::UnixUnnamed),
            },
            l.opts,
        );
        match addr {
            Addr::Inet { ip, .. } => {
                self.autobind(h, ip)?;
                // Bound to any address: the connection leaves from `ip`.
                if let Some(Addr::Inet { ip: mine, .. }) = &mut self.get_mut(h)?.local
                    && *mine == ANY
                {
                    *mine = ip;
                }
            }
            _ => {
                let s = self.get_mut(h)?;
                if s.local.is_none() {
                    s.local = Some(Addr::UnixUnnamed);
                }
            }
        }
        let mut server = Socket::new(family, Type::Stream, opts, net);
        server.refs = 0;
        server.local = Some(server_local);
        server.remote = self.get(h)?.local.clone();
        server.state = State::Connected { peer: Some(h) };
        let server = self.insert(server)?;
        let s = self.get_mut(h)?;
        s.state = State::Connected { peer: Some(server) };
        s.remote = Some(addr);
        if let State::Listening { queue, .. } = &mut self.get_mut(listener)?.state {
            queue.push_back(server);
        }
        self.touch(listener);
        Ok(())
    }

    /// `connect` to `ip:port` outside, through the network card: the first
    /// call starts it (`InProgress`), a call made again reports it.
    fn connect_out(&mut self, h: Handle, ty: Type, ip: [u8; 4], port: u16) -> Result<()> {
        let proto = match ty {
            Type::Stream | Type::SeqPacket => Proto::Tcp,
            Type::Dgram => Proto::Udp,
        };
        if !self.allows(self.get(h)?.net, proto, ip, port) {
            return Err(self.deny(proto, ip, port));
        }
        if self.wire_addr().is_none() {
            return Err(Error::NetUnreach);
        }
        self.went_out();
        if ty == Type::Dgram {
            self.autobind(h, ANY)?;
            self.get_mut(h)?.remote = Some(Addr::Inet { ip, port });
            return Ok(());
        }
        self.settle(h);
        let s = self.get_mut(h)?;
        match &mut s.state {
            State::Open => self.wire_connect(h, ip, port),
            State::Wire {
                ever_open, report, ..
            } => match (*ever_open, *report) {
                (false, _) => Err(Error::Again),
                (true, true) => {
                    *report = false;
                    Ok(())
                }
                (true, false) => Err(Error::IsConn),
            },
            State::Reset { pending: true } => {
                s.state = State::Reset { pending: false };
                Err(if s.refused {
                    Error::ConnRefused
                } else {
                    Error::ConnReset
                })
            }
            State::Listening { .. } => Err(Error::Invalid),
            _ => Err(Error::IsConn),
        }
    }

    /// Brings a connection through the card up to date: open once it
    /// opened, reset (or refused) once `smoltcp` closed it without the cell.
    fn settle(&mut self, h: Handle) {
        let Ok(s) = self.get(h) else { return };
        let State::Wire {
            handle, ever_open, ..
        } = s.state
        else {
            return;
        };
        let closed_by_cell = s.shut_wr;
        match self.wire_progress(handle) {
            Progress::Connecting => {}
            Progress::Open => {
                if let Ok(Socket {
                    state: State::Wire { ever_open, .. },
                    ..
                }) = self.get_mut(h)
                {
                    *ever_open = true;
                }
            }
            Progress::Failed => {
                self.wire_close(handle);
                if let Ok(s) = self.get_mut(h) {
                    s.state = State::Reset {
                        pending: !closed_by_cell,
                    };
                    s.refused = !ever_open;
                    s.eof = true;
                }
                self.touch(h);
            }
        }
    }

    /// The error a reset socket reports first.
    fn reset_error(&self, h: Handle) -> Error {
        match self.get(h) {
            Ok(s) if s.refused => Error::ConnRefused,
            _ => Error::ConnReset,
        }
    }

    /// `send`/`sendto`: queues bytes of `data` for the peer (a stream:
    /// as many as fit, `Again` if none fits) or one datagram to `to` or the
    /// default destination (dropped if nobody is bound there or its queue is
    /// full, as UDP does).
    pub fn send(&mut self, h: Handle, data: &[u8], to: Option<&Addr>) -> Result<usize> {
        self.look(h);
        let s = self.get_mut(h)?;
        if s.ty == Type::Dgram {
            return self.send_datagram(h, data, to);
        }
        if s.shut_wr {
            return Err(Error::Pipe);
        }
        self.settle(h);
        let s = self.get_mut(h)?;
        let peer = match (reset_pending(&mut s.state), &s.state) {
            (Some(true), _) => return Err(self.reset_error(h)),
            (Some(false), _) => return Err(Error::Pipe),
            (None, State::Connected { peer }) => peer.ok_or(Error::Pipe)?,
            (
                None,
                &State::Wire {
                    handle, ever_open, ..
                },
            ) => {
                return if ever_open {
                    self.wire_send(handle, data)
                } else {
                    Err(Error::Again)
                };
            }
            _ => return Err(Error::NotConn),
        };
        let p = self.get_mut(peer)?;
        let room = STREAM_BUF - p.rx.len();
        if room == 0 {
            return Err(Error::Again);
        }
        let n = room.min(data.len());
        p.rx.extend(&data[..n]);
        self.touch(peer);
        Ok(n)
    }

    fn send_datagram(&mut self, h: Handle, data: &[u8], to: Option<&Addr>) -> Result<usize> {
        let s = self.get(h)?;
        if s.shut_wr {
            return Err(Error::Pipe);
        }
        let (ip, port) = match to.or(s.remote.as_ref()) {
            Some(&Addr::Inet { ip, port }) => (if ip == ANY { LOOPBACK } else { ip }, port),
            Some(_) => return Err(Error::Invalid),
            None => return Err(Error::DestAddrReq),
        };
        if data.len() > DGRAM_MAX {
            return Err(Error::MsgSize);
        }
        if !is_loopback(ip) {
            // Every datagram out is checked, so a narrowed policy holds at
            // once for connected sockets too.
            if !self.allows(s.net, Proto::Udp, ip, port) {
                return Err(self.deny(Proto::Udp, ip, port));
            }
            self.went_out();
            self.autobind(h, ANY)?;
            self.wire_send_to(h, data, ip, port)?;
            return Ok(data.len());
        }
        self.autobind(h, ANY)?;
        let net = self.get(h)?.net;
        let from = match self.get(h)?.local {
            Some(Addr::Inet { ip: mine, port }) => Addr::Inet {
                ip: if mine == ANY { ip } else { mine },
                port,
            },
            _ => return Err(Error::Invalid),
        };
        let receiver = self.sockets.iter_mut().enumerate().find_map(|(i, r)| {
            let r = r.as_mut().filter(|r| r.net == net)?;
            let bound = r.ty == Type::Dgram
                && r.port()
                    .is_some_and(|(rip, rport)| rport == port && (rip == ANY || rip == ip))
                && r.remote.as_ref().is_none_or(|only| *only == from);
            bound.then_some((i as Handle, r))
        });
        if let Some((i, r)) = receiver
            && r.dgram_bytes + data.len() <= DGRAM_BUF
            && !r.shut_rd
        {
            r.dgram_bytes += data.len();
            r.dgrams.push_back((from, data.to_vec()));
            self.touch(i);
        }
        Ok(data.len())
    }

    /// `recv`/`recvfrom`: hands `take` up to `len` bytes, in pieces (a
    /// stream) or the front datagram cut to `len`; what `take` accepts is
    /// consumed unless `peek`. An empty stream is `Ok` with 0 at its end,
    /// `Again` before.
    pub fn recv(
        &mut self,
        h: Handle,
        len: usize,
        peek: bool,
        take: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Result<Received> {
        self.look(h);
        self.settle(h);
        let s = self.get_mut(h)?;
        if s.ty == Type::Dgram && s.dgrams.is_empty() && !s.shut_rd {
            if s.udp.is_some() {
                self.drop_denied(h);
            }
            let s = self.get_mut(h)?;
            if let Some(udp) = s.udp
                && let Some(got) = self.wire_recv_from(udp, len, peek, take)
            {
                return Ok(got);
            }
            return Err(Error::Again);
        }
        let s = self.get_mut(h)?;
        if s.ty == Type::Dgram {
            let Some((from, data)) = s.dgrams.front() else {
                return if s.shut_rd {
                    Ok(Received {
                        len: 0,
                        full: 0,
                        from: None,
                    })
                } else {
                    Err(Error::Again)
                };
            };
            let n = len.min(data.len());
            if !take(&data[..n]) {
                return Ok(Received {
                    len: 0,
                    full: 0,
                    from: None,
                });
            }
            let got = Received {
                len: n,
                full: data.len(),
                from: Some(from.clone()),
            };
            if !peek {
                s.dgram_bytes -= data.len();
                s.dgrams.pop_front();
            }
            return Ok(got);
        }
        match (reset_pending(&mut s.state), &s.state) {
            (Some(true), _) => return Err(self.reset_error(h)),
            (Some(false), _) => {
                return Ok(Received {
                    len: 0,
                    full: 0,
                    from: None,
                });
            }
            (None, State::Connected { .. }) => {}
            (
                None,
                &State::Wire {
                    handle, ever_open, ..
                },
            ) => {
                let shut = s.shut_rd;
                let len = if !ever_open {
                    return Err(Error::Again);
                } else if shut {
                    0
                } else {
                    self.wire_recv(handle, len, peek, take)?
                };
                return Ok(Received {
                    len,
                    full: len,
                    from: None,
                });
            }
            _ => return Err(Error::NotConn),
        }
        if s.rx.is_empty() {
            return if s.eof || s.shut_rd {
                Ok(Received {
                    len: 0,
                    full: 0,
                    from: None,
                })
            } else {
                Err(Error::Again)
            };
        }
        let mut done = 0;
        let (a, b) = s.rx.as_slices();
        for piece in [a, b] {
            let n = piece.len().min(len - done);
            if n == 0 || !take(&piece[..n]) {
                break;
            }
            done += n;
        }
        if !peek && done > 0 {
            // A writer waits only on a full queue.
            let full = s.rx.len() == STREAM_BUF;
            s.rx.drain(..done);
            if let (true, &State::Connected { peer: Some(p) }) = (full, &s.state) {
                self.touch(p);
            }
        }
        Ok(Received {
            len: done,
            full: done,
            from: None,
        })
    }

    /// `shutdown`.
    pub fn shutdown(&mut self, h: Handle, how: Shutdown) -> Result<()> {
        self.settle(h);
        let s = self.get_mut(h)?;
        let peer = match s.state {
            State::Connected { peer } => peer,
            State::Open if s.ty == Type::Dgram => None,
            State::Wire { handle, .. } => {
                if how != Shutdown::Read && !s.shut_wr {
                    s.shut_wr = true;
                    self.wire_shutdown(handle);
                }
                let s = self.get_mut(h)?;
                if how != Shutdown::Write {
                    s.shut_rd = true;
                }
                self.touch(h);
                return Ok(());
            }
            _ => return Err(Error::NotConn),
        };
        if how != Shutdown::Write {
            s.shut_rd = true;
        }
        if how != Shutdown::Read {
            s.shut_wr = true;
            if let Some(p) = peer
                && let Ok(ps) = self.get_mut(p)
            {
                ps.eof = true;
                self.touch(p);
            }
        }
        self.touch(h);
        Ok(())
    }

    /// The `poll` events of `h` (`events::*`).
    pub fn poll(&self, h: Handle) -> u32 {
        self.look(h);
        let Ok(s) = self.get(h) else {
            return ERR | HUP;
        };
        match &s.state {
            State::Open if s.ty == Type::Dgram => {
                let wire = s.udp.is_some_and(|u| self.wire_udp_ready(u));
                OUT | if s.dgrams.is_empty() && !s.shut_rd && !wire {
                    0
                } else {
                    IN
                }
            }
            &State::Wire { handle, .. } => match self.wire_progress(handle) {
                Progress::Connecting => 0,
                Progress::Failed => IN | OUT | HUP | RDHUP | ERR,
                Progress::Open => self.wire_poll(handle) | if s.shut_rd { IN } else { 0 },
            },
            State::Open => OUT | HUP,
            State::Listening { queue, .. } => {
                if queue.is_empty() {
                    0
                } else {
                    IN
                }
            }
            State::Reset { pending } => IN | OUT | HUP | RDHUP | if *pending { ERR } else { 0 },
            State::Endpoint { end, .. } => self.end_events(*end),
            State::Connected { peer } => {
                let mut ev = 0;
                if !s.rx.is_empty() || s.eof || s.shut_rd {
                    ev |= IN;
                }
                if s.eof {
                    ev |= RDHUP;
                }
                if s.eof && (s.shut_wr || peer.is_none()) {
                    ev |= HUP;
                }
                let room = peer.is_none_or(|p| self.get(p).is_ok_and(|p| p.rx.len() < STREAM_BUF));
                if !s.shut_wr && room {
                    ev |= OUT;
                }
                ev
            }
        }
    }

    /// The address `h` is bound to (`getsockname`).
    pub fn local_addr(&self, h: Handle) -> Result<Option<Addr>> {
        Ok(self.get(h)?.local.clone())
    }

    /// The address of `h`'s peer (`getpeername`).
    pub fn peer_addr(&self, h: Handle) -> Result<Addr> {
        let s = self.get(h)?;
        match (&s.state, &s.remote) {
            (State::Connected { .. } | State::Wire { .. }, Some(a)) => Ok(a.clone()),
            (State::Endpoint { .. }, _) => Ok(Addr::UnixUnnamed),
            (State::Open, Some(a)) if s.ty == Type::Dgram => Ok(a.clone()),
            _ => Err(Error::NotConn),
        }
    }

    /// The options of `h`.
    pub fn options(&self, h: Handle) -> Result<Options> {
        Ok(self.get(h)?.opts)
    }

    /// Changes the options of `h`.
    pub fn set_options(&mut self, h: Handle, opts: Options) -> Result<()> {
        let s = self.get_mut(h)?;
        s.opts = opts;
        // A connection already on the card: `TCP_NODELAY` applies at once,
        // as it does on Linux (HTTP clients set it after connecting).
        if let State::Wire { handle, .. } = s.state {
            self.wire_no_delay(handle, opts.no_delay);
        }
        Ok(())
    }

    /// `SO_ERROR`: the pending error of `h`, cleared by reading it.
    pub fn take_error(&mut self, h: Handle) -> Result<Option<Error>> {
        self.settle(h);
        let error = self.reset_error(h);
        let reset = reset_pending(&mut self.get_mut(h)?.state);
        Ok((reset == Some(true)).then_some(error))
    }

    /// The socket a copy of a cell gets for `h` in the copy's network `net`
    /// (design D4 of `add-sockets-and-poll`, D5 of `add-cell-hierarchy`): a
    /// listener or a bound socket is bound again at its address there (and
    /// listens again, with no connection waiting), as a resume does; a
    /// fresh one is copied; a connection (or one that was) is reset. `h` is
    /// unchanged: the source keeps its sockets, and nobody shares a
    /// listener.
    pub fn fork(&mut self, h: Handle, net: u32) -> Result<Handle> {
        let s = self.get(h)?;
        let mut copy = Socket::new(s.family, s.ty, s.opts, net);
        let again = match &s.state {
            // An inbox is its cell's: a copy has its own, and the one it
            // inherited reads as closed, as an endpoint does.
            State::Listening { backlog, .. } if s.ty != Type::SeqPacket => {
                Some((s.local.clone(), Some(*backlog)))
            }
            State::Open => {
                copy.remote = s.remote.clone();
                Some((s.local.clone(), None))
            }
            _ => {
                // An endpoint reads as closed in a copy (design D13 of
                // `add-cell-hierarchy`); a connection reports its reset.
                copy.state = State::Reset {
                    pending: s.ty != Type::SeqPacket,
                };
                copy.eof = true;
                None
            }
        };
        let c = self.insert(copy)?;
        if let Some((local, backlog)) = again {
            self.bind_again(c, local.as_ref(), backlog);
        }
        Ok(c)
    }

    /// Binds new socket `h` at `local` and makes it listen with `backlog`
    /// if given, in its own network; reset if that fails.
    fn bind_again(&mut self, h: Handle, local: Option<&Addr>, backlog: Option<usize>) {
        let bound = match local {
            Some(Addr::UnixUnnamed) | None => Ok(()),
            Some(addr) => self.bind(h, addr),
        };
        let ready = bound.and_then(|()| match backlog {
            Some(backlog) => self.listen(h, backlog),
            None => Ok(()),
        });
        if ready.is_err()
            && let Ok(s) = self.get_mut(h)
        {
            s.state = State::Reset { pending: true };
        }
    }

    /// Moves parked socket `h` and the connections waiting on it (if it
    /// listens) into network `net`.
    fn move_to(&mut self, h: Handle, net: u32) {
        let Ok(s) = self.get_mut(h) else { return };
        s.net = net;
        if let State::Listening { queue, .. } = &s.state {
            for q in queue.clone() {
                if let Ok(q) = self.get_mut(q) {
                    q.net = net;
                }
            }
        }
    }

    /// What a snapshot keeps of `h` (design D4 of `add-sockets-and-poll`):
    /// a listener or a connection is also parked, with one more reference,
    /// until [`import`](Self::import) or [`unpark`](Self::unpark) takes it
    /// back (design D1 of `keep-tcp-across-suspend`).
    pub fn export(&mut self, h: Handle) -> Result<Record> {
        let s = self.get(h)?;
        let inbox = s.ty == Type::SeqPacket;
        let (state, local) = match &s.state {
            State::Open => (Saved::Open, s.local.clone()),
            State::Listening { backlog, .. } if !inbox => {
                (Saved::Listening(*backlog as u32), s.local.clone())
            }
            _ => (Saved::Reset, None),
        };
        let mut record = Record {
            family: s.family,
            ty: s.ty,
            state,
            local,
            opts: s.opts,
            parked: None,
        };
        if !inbox
            && matches!(
                s.state,
                State::Listening { .. } | State::Connected { .. } | State::Wire { .. }
            )
        {
            // A key no other parked socket has, unpredictable without the
            // secret.
            let key = loop {
                let key = siphash24(self.secret, self.next_key);
                self.next_key += 1;
                if self.parked.iter().all(|p| p.0 != key) {
                    break key;
                }
            };
            self.parked.push((key, h));
            self.retain(h);
            record.parked = Some(Park {
                boot: self.boot,
                key,
            });
        }
        Ok(record)
    }

    /// The socket parked under `r` in this boot, taken out of the parked
    /// ones with its reference.
    fn take_parked(&mut self, r: &Record) -> Option<Handle> {
        let park = r.parked?;
        let i = self.parked.iter().position(|p| p.0 == park.key)?;
        (park.boot == self.boot).then(|| self.parked.swap_remove(i).1)
    }

    /// A socket made from snapshot record `r` in network `net`, the
    /// resumed cell's: one parked in this boot comes back as it is now
    /// (once), moved into `net`; otherwise a connection is reset and others
    /// are bound and listening again where they were (reset if their
    /// address is taken). A Unix name gets a new number: returned as `(old,
    /// new)` so the cell's files can follow (the same number for a listener
    /// that came back).
    pub fn import(&mut self, r: &Record, net: u32) -> Result<(Handle, Option<(u64, u64)>)> {
        if let Some(h) = self.take_parked(r) {
            self.move_to(h, net);
            let s = self.get(h)?;
            let name = match (&s.state, &s.local) {
                (State::Listening { .. }, Some(Addr::Unix { name, .. })) => Some((*name, *name)),
                _ => None,
            };
            return Ok((h, name));
        }
        let mut s = Socket::new(r.family, r.ty, r.opts, net);
        if r.state == Saved::Reset {
            s.state = State::Reset {
                pending: r.ty != Type::SeqPacket,
            };
            s.eof = true;
        }
        let h = self.insert(s)?;
        let mut renamed = None;
        let local = match &r.local {
            Some(Addr::Unix { name, path }) => {
                let new = self.new_name();
                renamed = Some((*name, new));
                Some(Addr::Unix {
                    name: new,
                    path: path.clone(),
                })
            }
            local => local.clone(),
        };
        let backlog = match r.state {
            Saved::Listening(backlog) => Some(backlog as usize),
            _ => None,
        };
        self.bind_again(h, local.as_ref(), backlog);
        Ok((h, renamed))
    }

    /// A snapshot holding record `r` is dropped: a socket parked under it
    /// closes.
    pub fn unpark(&mut self, r: &Record) {
        if let Some(h) = self.take_parked(r) {
            self.release(h);
        }
    }
}

/// SipHash-2-4 of one 64-bit word (little endian) under `key` (Aumasson
/// and Bernstein, "SipHash: a fast short-input PRF", 2012): a pseudorandom
/// function, so keys made from consecutive counters say nothing about each
/// other or about `key`.
fn siphash24(key: [u64; 2], m: u64) -> u64 {
    let mut v = [
        key[0] ^ 0x736f_6d65_7073_6575,
        key[1] ^ 0x646f_7261_6e64_6f6d,
        key[0] ^ 0x6c79_6765_6e65_7261,
        key[1] ^ 0x7465_6462_7974_6573,
    ];
    fn round(v: &mut [u64; 4]) {
        v[0] = v[0].wrapping_add(v[1]);
        v[1] = v[1].rotate_left(13) ^ v[0];
        v[0] = v[0].rotate_left(32);
        v[2] = v[2].wrapping_add(v[3]);
        v[3] = v[3].rotate_left(16) ^ v[2];
        v[0] = v[0].wrapping_add(v[3]);
        v[3] = v[3].rotate_left(21) ^ v[0];
        v[2] = v[2].wrapping_add(v[1]);
        v[1] = v[1].rotate_left(17) ^ v[2];
        v[2] = v[2].rotate_left(32);
    }
    // One message block, then the final block: no bytes left, length 8.
    for block in [m, 8 << 56] {
        v[3] ^= block;
        round(&mut v);
        round(&mut v);
        v[0] ^= block;
    }
    v[2] ^= 0xff;
    for _ in 0..4 {
        round(&mut v);
    }
    v[0] ^ v[1] ^ v[2] ^ v[3]
}

#[cfg(test)]
mod endpoint_tests;
#[cfg(test)]
mod hostile;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod wire_tests;
