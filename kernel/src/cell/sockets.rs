//! The kernel's sockets, shared by every cell (spec `networking`):
//! their lock, and what a cell's sockets become when it forks, is
//! suspended or resumed.

use super::*;

/// The sockets of every cell (spec `networking`): cells of this kernel talk
/// over its loopback network.
pub(super) static NET: Global<Net> = Global::new(Net::new());

/// Set, under the sockets' lock, when a change woke waiters that nobody
/// drained yet: the scheduler takes the lock to drain them only then, not
/// at every take and give-back of a cell (design D2 of
/// `add-socket-wait-queues`).
pub(super) static WOKEN: AtomicBool = AtomicBool::new(false);

/// The sockets as a system call reaches them (design D1 of
/// `split-sockets-lock`): locked the first time the call uses them, until
/// the call ends, with their change count then; a call that uses no socket
/// never waits for another CPU's socket calls.
#[derive(Default)]
pub(super) struct CallNet(pub(super) Option<(rhizo_core::sync::Guard<'static, Net>, u64)>);

impl rhizo_abi::Sockets for CallNet {
    fn net(&mut self) -> &mut Net {
        &mut self
            .0
            .get_or_insert_with(|| {
                let net = NET.lock();
                let changes = net.changes();
                (net, changes)
            })
            .0
    }
}

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

/// The files of a fork of a cell with `fs`: its sockets as design D4 of
/// `add-sockets-and-poll` says (listeners shared, connections reset).
pub(super) fn fork_files(fs: &Fs) -> Fs {
    let mut child = fs.clone();
    if fs.sockets().next().is_some() {
        with_net(|net| child.map_sockets(|h| net.fork(h).unwrap_or(h)));
    }
    child
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
/// suspended), leaving their wait queues first.
pub(super) fn release_sockets(fs: &Fs, id: u32) {
    if fs.sockets().next().is_some() {
        with_net(|net| {
            fs.sockets().for_each(|h| {
                net.forget(h, id);
                net.release(h);
            })
        });
    }
}

/// `fs` with its sockets replaced by indices into the records returned, for
/// a snapshot; its connections are parked in the kernel (design D1 of
/// `keep-tcp-across-suspend`).
pub(super) fn export_sockets(fs: &Fs) -> (Fs, Vec<rhizo_net::Record>) {
    let mut records = Vec::new();
    if fs.sockets().next().is_none() {
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

/// Makes the sockets of a resumed cell from its snapshot's records (its
/// parked connections come back), and points its descriptors and Unix
/// socket names at them. A socket that
/// cannot be made at all is left out: its descriptor reads as reset.
pub(super) fn import_sockets(fs: &mut Fs, records: &[rhizo_net::Record]) {
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
            let made = net.import(r).or_else(|_| net.import(&reset));
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
