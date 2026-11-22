//! The kernel's sockets (spec `networking`): their lock, and what a cell's
//! sockets become when it forks, is suspended or resumed. Each cell has a
//! loopback network of its own, numbered by its identifier (design D5 of
//! `add-cell-hierarchy`), with the egress policy its node in the tree holds
//! (design D7): `None` unless the host gives it one.

use super::*;

/// The sockets of every cell (spec `networking`), each in its cell's own
/// loopback network.
pub(super) static NET: Global<Net> = Global::new(Net::new());

/// Whether a network has an egress policy other than `None`, set under the
/// sockets' lock: until one has, a cell's end takes no lock for its network
/// when it holds no socket.
static POLICIES: AtomicBool = AtomicBool::new(false);

impl Host {
    /// Sets the egress policy of cell `id` (and, narrowed with it, of its
    /// descendants), as the host (design D7 of `add-cell-hierarchy`): a
    /// root's freely, another cell's within its parent's. Connections the
    /// new policies deny are reset. `false` if refused (no such cell, or a
    /// child asking for more than its parent has).
    #[cfg_attr(
        not(any(feature = "ktest", feature = "bench-cell")),
        allow(dead_code, reason = "used by in-kernel tests and benchmarks")
    )]
    pub fn set_egress(&mut self, id: u32, egress: rhizo_cap::Egress) -> bool {
        let ceiling = egress.clone();
        self.set_policy(id, egress, ceiling, rhizo_cap::Actor::Host)
            .is_ok()
    }

    /// Gives the networks of the cells of `id`'s subtree the egress
    /// policies their nodes now hold (after a placement, a re-parenting or a
    /// change of policy): connections a narrowed policy denies are reset.
    pub(super) fn sync_networks(&self, id: u32) {
        let policies: Vec<(u32, rhizo_cap::Egress)> = self
            .tree
            .get()
            .subtree(id)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|c| Some((c, self.tree.get().get(c)?.egress().clone())))
            .collect();
        let none = policies.iter().all(|(_, e)| *e == rhizo_cap::Egress::None);
        if none && !POLICIES.load(Ordering::Acquire) {
            return; // nothing to set, nothing to clear
        }
        with_net(|net| {
            for (c, egress) in policies {
                net.set_egress(c, egress);
            }
            POLICIES.store(net.has_policies(), Ordering::Release);
        });
    }

    /// Gives the networks of copies `ids` the egress policies of their
    /// nodes (their source's); those with `None` need nothing. While no
    /// network has a policy, no source has one: nothing to look up.
    pub(super) fn enter_networks(&self, ids: impl Iterator<Item = u32>) {
        if !POLICIES.load(Ordering::Acquire) {
            return;
        }
        let policies: Vec<(u32, rhizo_cap::Egress)> = ids
            .filter_map(|id| Some((id, self.tree.get().get(id)?.egress().clone())))
            .filter(|(_, egress)| *egress != rhizo_cap::Egress::None)
            .collect();
        if !policies.is_empty() {
            with_net(|net| {
                for (id, egress) in policies {
                    net.set_egress(id, egress);
                }
                POLICIES.store(true, Ordering::Release);
            });
        }
    }
}

/// Set, under the sockets' lock, when a change woke waiters that nobody
/// drained yet: the scheduler takes the lock to drain them only then, not
/// at every take and give-back of a cell (design D2 of
/// `add-socket-wait-queues`).
pub(super) static WOKEN: AtomicBool = AtomicBool::new(false);

/// The sockets as a system call reaches them (design D1 of
/// `split-sockets-lock`): locked the first time the call uses them, until
/// the call ends, with their change count then, and the grant table for a
/// call that moves handles (module `handles`); a call that uses no socket
/// never waits for another CPU's socket calls.
#[derive(Default)]
pub(super) struct CallNet(pub(super) Option<CallState>);

/// Runs `f` on the sockets, noting woken waiters for [`wake_woken`].
pub(super) fn with_net<R>(f: impl FnOnce(&mut Net) -> R) -> R {
    NET.with(|net| {
        let r = f(net);
        if net.has_woken() {
            WOKEN.store(true, Ordering::Release);
        }
        r
    })
}

/// The files of copy `id` of a cell with `fs`: its sockets in the copy's
/// own network (design D5 of `add-cell-hierarchy`: listeners and bound
/// sockets bound again there, connections reset). A socket that cannot be
/// made reads as reset.
pub(super) fn fork_files(mut child: Fs, id: u32) -> Fs {
    if child.has_sockets() {
        with_net(|net| child.map_sockets(|h| net.fork(h, id).unwrap_or(u32::MAX)));
    }
    child
}

/// Sets the host's deny floor (`rhizo_net::Net::set_floor`, protocol
/// version 4's `floor`): `deny` beyond the card's gateway and name servers,
/// which are always in it, and the endpoints `carved` out. Connections it
/// now denies are reset. `false` if it holds too many rules.
#[cfg_attr(
    not(feature = "control"),
    allow(dead_code, reason = "used by the control kernel")
)]
pub fn set_floor(deny: &[rhizo_net::Rule], carved: &[rhizo_net::Rule]) -> bool {
    with_net(|net| net.set_floor(deny, carved)).is_ok()
}

/// Sockets open in the kernel.
#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests")
)]
pub fn open_sockets() -> usize {
    with_net(|net| net.open())
}

/// Whether connections through the network card are still open or
/// closing (`smoltcp` keeps a closed connection until its peer has
/// acknowledged the close).
#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests")
)]
pub fn wire_busy() -> bool {
    with_net(|net| net.wire_busy())
}

/// Sets how long connections through the network card wait for an answer
/// before they give up, from now on (`rhizo_net::Net::set_wire_timeout`).
#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests")
)]
pub fn set_wire_timeout(ns: u64) {
    with_net(|net| net.set_wire_timeout(ns));
}

/// Closes the connections parked for `snapshot`, which the host drops
/// without resuming it (spec `networking`, design D3 of
/// `keep-tcp-across-suspend`); resumed later, it finds them reset.
#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests")
)]
pub fn close_parked(snapshot: &Snapshot) {
    let records = snapshot.sockets();
    if !records.is_empty() {
        with_net(|net| records.iter().for_each(|r| net.unpark(r)));
    }
}

/// Releases the sockets `fs` of cell `id` holds (it ended or was
/// suspended), leaving their wait queues first. Its network's policy goes
/// when `network_ends`; a suspended cell's stays for its resume
/// ([`resume_network`]), or until its snapshot is dropped
/// ([`forget_network`]).
pub(super) fn release_sockets(fs: &Fs, id: u32, network_ends: bool) {
    let policy = network_ends && POLICIES.load(Ordering::Acquire);
    if fs.has_sockets() || policy {
        with_net(|net| {
            fs.sockets().for_each(|h| {
                net.forget(h, id);
                net.release(h);
            });
            if policy {
                net.drop_network(id);
                POLICIES.store(net.has_policies(), Ordering::Release);
            }
        });
    }
}

/// Whether a network has an egress policy other than `None`.
pub(super) fn has_policies() -> bool {
    POLICIES.load(Ordering::Acquire)
}

/// Suspended cell `old` was resumed as `new`: its network's policy follows.
pub(super) fn resume_network(old: u32, new: u32) {
    with_net(|net| net.rename_network(old, new));
}

/// Cell `id` is gone for good (a dropped snapshot, a cascade): its
/// network's policy goes. Nothing to do while no network has one.
pub(super) fn forget_network(id: u32) {
    if POLICIES.load(Ordering::Acquire) {
        with_net(|net| {
            net.drop_network(id);
            POLICIES.store(net.has_policies(), Ordering::Release);
        });
    }
}

/// `fs` with its sockets replaced by indices into the records returned, for
/// a snapshot; its connections are parked in the kernel (design D1 of
/// `keep-tcp-across-suspend`).
pub(super) fn export_sockets(fs: &Fs) -> (Fs, Vec<rhizo_net::Record>) {
    let mut records = Vec::new();
    if !fs.has_sockets() {
        return (fs.clone(), records);
    }
    let mut out = fs.clone();
    with_net(|net| {
        out.map_sockets(|h| {
            let record = net.export(h).unwrap_or(rhizo_net::Record {
                family: rhizo_net::Family::Inet,
                ty: rhizo_net::Type::Stream,
                state: rhizo_net::Saved::Reset,
                local: None,
                opts: rhizo_net::Options::default(),
                parked: None,
            });
            records.push(record);
            (records.len() - 1) as u32
        });
    });
    (out, records)
}

/// Makes the sockets of resumed cell `id` from its snapshot's records, in
/// its network (its parked connections come back), and points its
/// descriptors and Unix socket names at them. A socket that cannot be made
/// at all is left out: its descriptor reads as reset.
pub(super) fn import_sockets(fs: &mut Fs, records: &[rhizo_net::Record], id: u32) {
    if records.is_empty() {
        return;
    }
    let mut handles = Vec::with_capacity(records.len());
    let mut renamed = Vec::new();
    with_net(|net| {
        for r in records {
            let reset = rhizo_net::Record {
                state: rhizo_net::Saved::Reset,
                local: None,
                parked: None,
                ..r.clone()
            };
            let made = net.import(r, id).or_else(|_| net.import(&reset, id));
            let (h, name) = made.unwrap_or((u32::MAX, None));
            handles.push(h);
            renamed.extend(name);
        }
    });
    fs.map_sockets(|i| handles.get(i as usize).copied().unwrap_or(u32::MAX));
    // A name keeps working only if its socket came back with the cell.
    if fs.socket_names().next().is_some() {
        fs.map_socket_names(|n| renamed.iter().find(|r| r.0 == n).map_or(0, |r| r.1));
    }
}

// A new cell's file tables start with its default socket quota (so making
// a live cell copies no tables), and with the server ends of its own
// connections it holds at most an eighth of the kernel's sockets (S21).
const _: () = assert!(rhizo_cell::quota::SOCKETS_DEFAULT as usize == rhizo_fs::SOCKETS_DEFAULT);
const _: () = assert!(2 * rhizo_fs::SOCKETS_DEFAULT <= rhizo_net::SOCKETS_MAX / 8);
