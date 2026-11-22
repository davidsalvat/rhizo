//! Handles in the host (spec `cell-hierarchy`, design D8 to D10 of
//! `add-cell-hierarchy`, task 2.5): the grant table beside the cell tree
//! and what system calls on any CPU reach of them, the host's grants and
//! revocations, control handles as authority over a cell, the input and
//! output cell handles carry, and what a cell's end does to its handles.
//!
//! The sockets' lock ([`NET`]) guards the grant table, which system calls
//! and the host change, and the tree against the host's changes while
//! other CPUs run cells, whose calls read it: a call that moves handles
//! already holds that lock, and takes no other. The host reads its tree
//! without a lock: it is its only writer.

use super::*;
use core::cell::UnsafeCell;
use core::sync::atomic::AtomicUsize;
use rhizo_cap::{Actor, Ask, Error as CapError, Grants, Label, Object, Rights, Tree};

/// The tree of a host ([`Host::tree`]): read by the host without a lock,
/// changed only through [`Host::tree_mut`], read by system calls under
/// the sockets' lock.
pub(super) struct TreeCell(UnsafeCell<Tree>);

impl core::fmt::Debug for TreeCell {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.get().fmt(f)
    }
}

impl TreeCell {
    pub(super) fn new() -> Self {
        Self(UnsafeCell::new(Tree::new()))
    }

    /// The tree.
    pub(super) fn get(&self) -> &Tree {
        // SAFETY: the tree changes only through `Host::tree_mut` (or
        // `get_mut`), which needs the host borrowed uniquely, so no
        // reference from here lives across a change on this CPU; other
        // CPUs only read it, while they hold the sockets' lock, which
        // `tree_mut` takes while they run cells.
        unsafe { &*self.0.get() }
    }

    /// The tree, with nobody else reaching it: no other CPU runs cells.
    pub(super) fn get_mut(&mut self) -> &mut Tree {
        self.0.get_mut()
    }
}

/// The grant table of a host ([`Host::grants`]): reached only under the
/// sockets' lock, by the host and by system calls alike.
pub(super) struct GrantsCell(UnsafeCell<Grants>);

impl core::fmt::Debug for GrantsCell {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GrantsCell").finish_non_exhaustive()
    }
}

impl GrantsCell {
    pub(super) fn new() -> Self {
        Self(UnsafeCell::new(Grants::new()))
    }

    /// The table, to whoever holds the sockets' lock: dereference it only
    /// while holding it, and make no other reference to it meanwhile.
    fn ptr(&self) -> *mut Grants {
        self.0.get()
    }

    /// The table, read under the sockets' lock.
    fn read<R>(&self, f: impl FnOnce(&Grants) -> R) -> R {
        NET.with(|_| {
            // SAFETY: the table is reached only under the sockets' lock,
            // held here for as long as `f` reads it.
            f(unsafe { &*self.ptr() })
        })
    }
}

/// The tree and grant table of the host whose cells run now (0: none),
/// lent by [`Host::lend`] for its runs: system calls reach them there.
static TREE_AT: AtomicUsize = AtomicUsize::new(0);
static GRANTS_AT: AtomicUsize = AtomicUsize::new(0);

/// Whether any cell ever had handle state in the kernel (a grant, an
/// inbox, an endpoint the kernel holds): until then a cell's end has no
/// handles to drop.
static HANDLES: AtomicBool = AtomicBool::new(false);
/// Whether the kernel holds an endpoint for some cell's observer: only
/// then does a cell's output go anywhere but its console.
static OBSERVED: AtomicBool = AtomicBool::new(false);
/// Whether any cell ever had handle state in the kernel.
pub(super) fn any_handles() -> bool {
    HANDLES.load(Ordering::Relaxed)
}

/// The tree and grant table lent for the current run, if any.
fn lent() -> Option<(&'static TreeCell, &'static GrantsCell)> {
    let (tree, grants) = (
        TREE_AT.load(Ordering::Acquire),
        GRANTS_AT.load(Ordering::Acquire),
    );
    if tree == 0 || grants == 0 {
        return None;
    }
    // SAFETY: `Host::lend` stores the addresses of its own fields before
    // it runs cells, and the host neither moves nor drops while cells run
    // (it is borrowed by `run` or `step` throughout); every system call
    // runs inside such a run.
    Some(unsafe { (&*(tree as *const TreeCell), &*(grants as *const GrantsCell)) })
}

/// The kernel's sockets as a system call reaches them: locked the first
/// time the call uses them, until the call ends (design D1 of
/// `split-sockets-lock`), with their change count then. Their lock guards
/// the tree and the grant table too.
pub(super) type CallState = (rhizo_core::sync::Guard<'static, Net>, u64);

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

    fn handles(&mut self) -> Option<rhizo_abi::Handles<'_>> {
        let (tree, grants) = lent()?;
        HANDLES.store(true, Ordering::Relaxed);
        let net = self.net();
        // SAFETY: the table is reached only under the sockets' lock, which
        // `self` holds until the call ends, longer than this borrow of it.
        let grants = unsafe { &mut *grants.ptr() };
        Some(rhizo_abi::Handles {
            tree: tree.get(),
            grants,
            net,
        })
    }

    fn output(&mut self, cell: u32, bytes: &[u8]) {
        if !OBSERVED.load(Ordering::Relaxed) {
            return;
        }
        let frame = rhizo_control::Message::Output {
            cell: 0,
            data: bytes.to_vec(),
        }
        .encode(0);
        let net = self.net();
        net.tell(cell, &frame);
        if !net.observed() {
            OBSERVED.store(false, Ordering::Relaxed);
        }
    }

    fn observing(&mut self) {
        HANDLES.store(true, Ordering::Relaxed);
        OBSERVED.store(true, Ordering::Relaxed);
    }
}

/// After a call of a cell that reached the sockets (`net`, with the grant
/// table if the call took it), when the sockets say so
/// (`rhizo_net::Net::take_after`): the cell received something from
/// another cell (its sticky bit, design D22), the room of channels it made
/// that ended comes back to it, and the grants whose last socket closed
/// are dropped.
#[cold]
#[inline(never)]
pub(super) fn after_handles(cell: &mut Cell, frames: &Meter, state: &mut CallState) {
    let net = &mut state.0;
    if net.take_received() {
        cell.exposed = true;
    }
    let ended = net.take_refunds(cell.id);
    if ended != 0 {
        frames.credit(u64::from(ended) * rhizo_cap::ROOM);
    }
    if net.has_dropped()
        && let Some((_, grants)) = lent()
    {
        // SAFETY: the table is reached only under the sockets' lock, which
        // `state` holds.
        rhizo_abi::drop_handed_back(unsafe { &mut *grants.ptr() }, net);
    }
}

/// Gives `live`, which a socket change woke while it ran, the input
/// waiting for it in endpoints the kernel holds (task 2.5): from the
/// scheduler's nudge, under its lock, then the sockets'.
#[cold]
#[inline(never)]
pub(super) fn take_input(live: &mut Live) {
    NET.with(|net| {
        if net.input_for(live.cell.id) {
            rhizo_abi::deliver_input(net, live.cell.id, &mut live.fs);
        }
    });
}

/// Gives every cell of `sched` that input waits for that input (task 2.5),
/// before the cells woken by socket changes are woken (a sender's message
/// wakes its cell): under the host's lock, then the sockets', as
/// [`wake_woken`] takes them. A cell running now is nudged instead, and
/// takes its input when it is next filed or taken ([`take_input`]).
#[inline(always)]
pub(super) fn deliver_waiting(sched: &mut Scheduler<Box<Live>, Option<Fair>>, net: &mut Net) {
    if net.has_input() {
        deliver_all(sched, net);
    }
}

#[cold]
#[inline(never)]
fn deliver_all(sched: &mut Scheduler<Box<Live>, Option<Fair>>, net: &mut Net) {
    let ids = net.input_cells().to_vec();
    for id in ids {
        if let Some(live) = sched.find_mut(id) {
            rhizo_abi::deliver_input(net, id, &mut live.fs);
        }
    }
}

/// What an observer reads last of a cell that ended this way (design D12,
/// item 37: its exit status goes to its observe handles); nothing for a
/// suspended cell, whose handles go with its suspension.
fn last_word(ending: Ending) -> Vec<u8> {
    use rhizo_control::State;
    let state = match ending {
        Ending::Exit(code) => State::Exited(code),
        Ending::Fault => State::Fault,
        Ending::Killed | Ending::OutOfCpu => State::Killed,
        Ending::Suspended => return Vec::new(),
    };
    rhizo_control::Message::Ended { cell: 0, state }.encode(0)
}

/// Makes `fs`, a cell's files, hold `/run/rhizo` with its generation and
/// inbox (design D9 of `add-cell-hierarchy`), before the cell runs.
/// Files every cell shares (an empty tree, the runtimes') get it once, so
/// a new cell copies nothing for it.
pub(super) fn rhizo_files(fs: &mut Fs) {
    if fs.has_rhizo() {
        return;
    }
    // A tree with no room for two entries refuses the cell's own files
    // first: the directory then reads as missing.
    let _ = fs.rhizo_dir();
    let _ = fs.rhizo_put(rhizo_fs::Place::Inbox, rhizo_fs::Kept::Inbox);
}

impl Host {
    /// Changes the tree: under the sockets' lock while other CPUs run
    /// cells, whose system calls read it then.
    pub(super) fn tree_mut<R>(&mut self, f: impl FnOnce(&mut Tree) -> R) -> R {
        let others = self.others;
        let tree = self.tree.get_mut();
        if others {
            NET.with(|_| f(tree))
        } else {
            f(tree)
        }
    }

    /// Runs `f` on the sockets, the tree and the grant table, under the
    /// sockets' lock, once the grants whose last socket closed are dropped:
    /// a revocation never meets the number of a grant handed back.
    pub(super) fn with_handles<R>(
        &mut self,
        f: impl FnOnce(&mut Net, &Tree, &mut Grants) -> R,
    ) -> R {
        HANDLES.store(true, Ordering::Relaxed);
        let (tree, grants) = (&self.tree, &mut self.grants);
        with_net(|net| {
            // The host's own reference, under the lock every other user of
            // the table takes.
            let g = grants.0.get_mut();
            rhizo_abi::drop_handed_back(g, net);
            f(net, tree.get(), g)
        })
    }

    /// Lends this host's tree and grant table to the system calls of its
    /// cells, before they run.
    pub(super) fn lend(&self) {
        TREE_AT.store(&self.tree as *const TreeCell as usize, Ordering::Release);
        GRANTS_AT.store(
            &self.grants as *const GrantsCell as usize,
            Ordering::Release,
        );
    }

    /// The host goes: what it lent with it.
    pub(super) fn lent_no_more(&self) {
        let me = &self.tree as *const TreeCell as usize;
        if TREE_AT
            .compare_exchange(me, 0, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            GRANTS_AT.store(0, Ordering::Release);
        }
    }

    /// Grants cell `holder` a handle on cell `target` with `rights` at
    /// `by`'s request, as its `/run/rhizo/cell/<label>` (design D9: the
    /// host grants any cell to any cell, a cell only a handle on a
    /// descendant to itself or a descendant); returns the grant. Refused
    /// as the grant table refuses it (control never to the cell it names
    /// or a descendant, the exposure check of design D8, the bounds), and
    /// with `NotFound` if `holder` is not live, `Exists` if the label is
    /// taken, `Invalid` if it is no label.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(
            dead_code,
            reason = "used by in-kernel tests until the protocol grants (task 2.7)"
        )
    )]
    pub fn grant(
        &mut self,
        by: Actor,
        holder: u32,
        target: u32,
        rights: Rights,
        label: &[u8],
    ) -> Result<u32, CapError> {
        let label = Label::new(label)?;
        if label.as_bytes().is_empty() {
            return Err(CapError::Invalid);
        }
        let ask = Ask {
            rights,
            holder,
            label: label.clone(),
            ..Ask::default()
        };
        // A taken label is refused before anything is granted.
        let mut path = b"/run/rhizo/cell/".to_vec();
        path.extend_from_slice(label.as_bytes());
        match self.with_live(holder, |l| l.fs.rhizo_entry(None, &path).is_ok()) {
            None => return Err(CapError::NotFound),
            Some(true) => return Err(CapError::Exists),
            Some(false) => {}
        }
        let object = Object::Cell(target);
        let g = self.with_handles(|net, tree, grants| {
            grants.grant(tree, net.channels(), by, object, ask)
        })?;
        let place = rhizo_fs::Place::Cell(label.as_bytes());
        let put = self.with_live(holder, |l| l.fs.rhizo_put(place, rhizo_fs::Kept::Grant(g)));
        let refused = match put {
            Some(Ok(())) => return Ok(g),
            Some(Err(rhizo_fs::Error::Exists)) => CapError::Exists,
            Some(Err(_)) => CapError::NoMemory,
            None => CapError::NotFound,
        };
        let _ = self.with_handles(|net, tree, grants| {
            grants.revoke(tree, Actor::Host, g, &mut |id, _| net.grant_gone(id))
        });
        Err(refused)
    }

    /// Revokes grant `g` and everything derived from it at `by`'s request
    /// (design D9: the host any grant, a cell those granted in its
    /// subtree): channel endpoints read as reset, cell handles refuse
    /// operations with `EACCES`, and what was opened through them ends.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(
            dead_code,
            reason = "used by in-kernel tests until the protocol revokes (task 2.7)"
        )
    )]
    pub fn revoke(&mut self, by: Actor, g: u32) -> Result<(), CapError> {
        self.with_handles(|net, tree, grants| {
            let mut gone = Vec::new();
            grants.revoke(tree, by, g, &mut |id, _| gone.push(id))?;
            gone.into_iter().for_each(|id| net.grant_gone(id));
            Ok(())
        })
    }

    /// Whether `by` controls cell `id` (design D2): the host, an ancestor,
    /// or a cell holding a handle with the control right on it.
    pub(super) fn controls(&self, by: Actor, id: u32) -> bool {
        self.tree.get().controls(by, id) || self.holds_control(by, id)
    }

    /// Whether cell `by` holds a handle with the control right on cell `id`.
    fn holds_control(&self, by: Actor, id: u32) -> bool {
        let Actor::Cell(a) = by else {
            return false;
        };
        self.grants
            .read(|g| g.holds(a, Object::Cell(id), Rights::CONTROL))
    }

    /// The authority under which an operation `by` asks for on cell `id`
    /// runs in the tree (design D2): `by`'s own when it is the host or an
    /// ancestor; for the holder of a control handle on `id`, that of
    /// `id`'s parent, never the host's (a root's handle holder gets
    /// `Denied` for what only the host may do); `Denied` otherwise.
    pub(super) fn acting(&self, by: Actor, id: u32) -> Result<Actor, CapError> {
        let tree = self.tree.get();
        let parent = tree.get(id).ok_or(CapError::NotFound)?.parent();
        if tree.controls(by, id) {
            return Ok(by);
        }
        match parent {
            rhizo_cap::HOST => Err(CapError::Denied),
            p if self.holds_control(by, id) => Ok(Actor::Cell(p)),
            _ => Err(CapError::Denied),
        }
    }

    /// Whether the handles still hold after the tree became `tree` around
    /// the cells `cells` (design D8, D2): no control handle on a cell held
    /// by it or a descendant, every exposure within its ceiling. Free while
    /// nobody holds a handle: the tree alone keeps every child within its
    /// parent's policy.
    pub(super) fn handles_admit(&self, tree: &Tree, cells: &[u32]) -> bool {
        if !HANDLES.load(Ordering::Relaxed) {
            return true;
        }
        self.grants
            .read(|g| g.iter().next().is_none() || g.admits(tree, cells).is_ok())
    }

    /// Cell `id` left (ended as `ending`, or was suspended): the handles on
    /// it are revoked and those it held dropped (a snapshot keeps none
    /// until format version 12, task 2.6), its observers read `ending`'s
    /// last word then end of file, its input senders' writes fail, and its
    /// inbox closes with the channels waiting there. Nothing to do while no
    /// cell ever had a handle.
    #[inline(always)]
    pub(super) fn cell_left(&mut self, id: u32, ending: Ending) {
        if HANDLES.load(Ordering::Relaxed) {
            self.handles_left(id, ending);
        }
    }

    /// [`cell_left`](Self::cell_left) once cells have handles.
    #[inline(never)]
    fn handles_left(&mut self, id: u32, ending: Ending) {
        let last = last_word(ending);
        self.with_handles(|net, _, grants| {
            // Its observers' last word first, before their handles go.
            net.cell_ended(id, &last);
            // The handles on it are revoked; of those it held, cell
            // handles are dropped (what was opened through them stays
            // until closed) and channel endpoints read as closed.
            let mut gone = Vec::new();
            grants.cell_gone(id, &mut |g, x| {
                let dropped = x.holder() == id && matches!(x.object(), Object::Cell(_));
                gone.push((g, dropped));
            });
            for (g, dropped) in gone {
                match dropped {
                    true => net.forget_grant(g),
                    false => net.grant_gone(g),
                }
            }
            OBSERVED.store(net.observed(), Ordering::Relaxed);
        });
    }
}
