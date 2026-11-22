//! The ownership tree of the host's cells (spec `cell-hierarchy`, design D1
//! to D3 of `add-cell-hierarchy`): where each cell goes when it is made,
//! its domain and its number there, and what its end does to its subtree.

use super::*;
use rhizo_cap::{Actor, Egress, Error as TreeError, HOST, Limits, Spec, Tree};

/// A fresh domain identifier: from the cells' own counter, so never 0 and
/// never one another domain had in this boot.
fn fresh_domain() -> u32 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Sockets the reserved roots of every host hold (design D4 of
/// `add-cell-hierarchy`, roots and admission): the sum of their socket
/// limits, at most half of `rhizo_net::SOCKETS_MAX` (a connection costs
/// two sockets).
pub(super) static RESERVED_SOCKETS: AtomicU32 = AtomicU32::new(0);
/// Cells the reserved roots of every host hold: one each with its
/// descendant limit, at most `rhizo_cap::limits::CELLS_MAX`.
pub(super) static RESERVED_CELLS: AtomicU32 = AtomicU32::new(0);

/// Most sockets the reserved roots may hold together.
const RESERVED_SOCKETS_MAX: u64 = (rhizo_net::SOCKETS_MAX / 2) as u64;

/// The limits of a root with `quota`.
fn root_limits(quota: &Quota) -> Limits {
    Limits {
        memory: quota.memory,
        cpu_ns: quota.cpu_ns,
        sockets: quota.sockets,
        cells: u32::MAX,
    }
}

/// Whether the kernel can take reserved roots holding `more` than they
/// do (design D4, roots and admission): memory that cells can take now,
/// less what is set aside for other reserved domains (the kernel's caches
/// dropped if need be), sockets within half of the kernel's and cells
/// within its cell table.
fn admits(more: Limits) -> bool {
    let sockets = u64::from(RESERVED_SOCKETS.load(Ordering::Relaxed)) + u64::from(more.sockets);
    let cells = u64::from(RESERVED_CELLS.load(Ordering::Relaxed)) + u64::from(more.cells);
    sockets <= RESERVED_SOCKETS_MAX
        && cells <= u64::from(rhizo_cap::limits::CELLS_MAX)
        && (more.memory == 0 || cells_may_take(more.memory))
}

impl Host {
    /// The tree of this host's cells.
    #[cfg_attr(
        not(any(feature = "ktest", feature = "control")),
        allow(dead_code, reason = "used by in-kernel tests and the control kernel")
    )]
    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    /// The domain of cell `id` (live, suspended or a template): what kernel
    /// state shared between cells is keyed by (`rhizo_cap::Node::domain`).
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn domain(&self, id: u32) -> Option<u32> {
        self.tree.domain(id)
    }

    /// Enters `cell`, new and not yet run, as the root of a new domain, its
    /// number 1 (`getpid`, spec `linux-abi`); `false` if the tree has no
    /// room.
    pub(super) fn enter_root(&mut self, cell: &mut Cell) -> bool {
        // A new cell is no reserved root: it may not take the room of the
        // kernel's cell table reserved roots hold (design D4).
        let reserved = u64::from(RESERVED_CELLS.load(Ordering::Relaxed));
        if self.tree.len() as u64 + reserved >= u64::from(rhizo_cap::limits::CELLS_MAX) {
            return false;
        }
        let spec = Spec {
            limits: root_limits(&cell.quota),
            ..Spec::default()
        };
        let Ok(pid) = self.tree.add_root(cell.id, spec, 0) else {
            return false;
        };
        cell.renumber(pid);
        cell.domain = cell.id;
        self.reshare_if_by_weight();
        true
    }

    /// Enters `copies`, made from cell `source` and not yet run, as its
    /// siblings (design D13): all, or none and `false`. They keep their
    /// threads' identifiers and take new numbers in their domain, never the
    /// source's (design D22).
    pub(super) fn enter_copies(&mut self, source: u32, copies: &mut [Box<Live>]) -> bool {
        // One copy, as most often, needs no list.
        let one = [copies.first().map_or(0, |c| c.cell.id)];
        let many: Vec<u32> = match copies.len() {
            1 => Vec::new(),
            _ => copies.iter().map(|c| c.cell.id).collect(),
        };
        let ids = if copies.len() == 1 {
            &one[..]
        } else {
            &many[..]
        };
        // Copies of a reserved root are reserved roots (admitted before
        // they were made, `admits_copies`): they have its memory limit.
        let limited = copies
            .first()
            .is_some_and(|c| c.cell.quota.memory != u64::MAX);
        let mut placed = |k: usize, pid, domain| {
            if let Some(copy) = copies.get_mut(k) {
                (copy.cell.pid, copy.cell.domain) = (pid, domain);
            }
        };
        self.add_copies(source, ids, limited, &mut placed)
    }

    /// Adds `ids` to the tree as copies of cell `source`, telling `placed`
    /// each one's number and domain: all, or none and `false`. `limited`:
    /// they have a memory limit.
    #[inline(never)]
    fn add_copies(
        &mut self,
        source: u32,
        ids: &[u32],
        limited: bool,
        placed: &mut dyn FnMut(usize, u32, u32),
    ) -> bool {
        let parent = self.tree.get(source).map_or(0, |n| n.parent());
        let used = self.usage(parent);
        let entered = self.tree.add_copies(source, ids, &|_| used, placed).is_ok();
        if entered {
            self.reshare_if_by_weight();
            if limited && parent == HOST {
                self.refresh_reserved();
            } else if parent != HOST {
                // Carved from their parent: it may use that much less.
                self.sync(parent);
            }
        }
        entered
    }

    /// Whether the kernel can take `n` copies of cell `source`: each copy
    /// of a reserved root is a reserved root holding its limits, admitted
    /// against the kernel's memory, sockets and cell table (design D4).
    /// Copies of other cells take nothing more of the kernel's.
    pub(super) fn admits_copies(&self, source: u32, n: usize) -> bool {
        let Some(node) = self.tree.get(source).filter(|n| n.is_root()) else {
            return true;
        };
        let one = node.limits().reservation();
        if one == Limits::ZERO {
            return true;
        }
        let n = n as u64;
        admits(Limits {
            memory: one.memory.saturating_mul(n),
            cpu_ns: 0,
            sockets: u32::try_from(u64::from(one.sockets) * n).unwrap_or(u32::MAX),
            cells: u32::try_from(u64::from(one.cells) * n).unwrap_or(u32::MAX),
        })
    }

    /// Whether the kernel can take cell `id` as a root with `limits` (a
    /// new root, or one whose limits change), against what it holds now:
    /// a reserved root's memory against what cells can take less what is
    /// set aside for others, its sockets against half of the kernel's and
    /// its descendants against the cell table (design D4, roots and
    /// admission). A root without a memory limit reserves nothing.
    fn admits_root(&mut self, id: u32, limits: Limits) -> bool {
        let new = limits.reservation();
        if new == Limits::ZERO {
            return true;
        }
        let old = self
            .tree
            .get(id)
            .filter(|n| n.is_root())
            .map_or(Limits::ZERO, |n| n.limits().reservation());
        // What its meter, or its suspension, sets aside now, and will.
        let aside = self.held_aside.iter().find(|&&(c, _)| c == id);
        let (used, limit) = match aside {
            Some(&(_, frames)) => {
                let limit = self.tree.get(id).map_or(0, |n| n.limits().memory) / FRAME_SIZE;
                (limit.saturating_sub(frames), limit)
            }
            None => self
                .with_live(id, |live| (live.frames.used(), live.frames.limit()))
                .unwrap_or((0, u64::MAX)),
        };
        let held = |limit: u64| match limit < Quota::NONE.frames() {
            true => limit.saturating_sub(used),
            false => 0,
        };
        let more = held(new.memory / FRAME_SIZE).saturating_sub(held(limit));
        admits(Limits {
            memory: more * FRAME_SIZE,
            cpu_ns: 0,
            sockets: new.sockets.saturating_sub(old.sockets),
            cells: new.cells.saturating_sub(old.cells),
        })
    }

    /// The kernel's reservations follow this host's reserved roots, after
    /// a change to them: sockets kept from cells without a memory limit,
    /// and room in the cell table.
    pub(super) fn refresh_reserved(&mut self) {
        let now = self.tree.reserved_roots();
        let was = core::mem::replace(&mut self.reserved, now);
        if (now.sockets, now.cells) == (was.sockets, was.cells) {
            return;
        }
        let shift = |counter: &AtomicU32, from: u32, to: u32| {
            counter.fetch_add(to.wrapping_sub(from), Ordering::Relaxed);
        };
        shift(&RESERVED_CELLS, was.cells, now.cells);
        shift(&RESERVED_SOCKETS, was.sockets, now.sockets);
        let sockets = RESERVED_SOCKETS.load(Ordering::Relaxed) as usize;
        with_net(|net| net.set_reserved(2 * sockets));
    }

    /// Records in `snapshot`, taken of cell `id`, its node's limits and
    /// policies (format version 11, spec `snapshots`, tree state).
    pub(super) fn record_tree(&self, snapshot: &mut Snapshot, id: u32) {
        let Some(n) = self.tree.get(id) else {
            return;
        };
        snapshot.set_tree(&n.spec());
    }

    /// Places a cell restored from `snapshot`, not yet run: under the
    /// parent it had, in its domain, if it was suspended from this host; as
    /// a copy of the cell it was taken of (a template's snapshot), a
    /// sibling in its domain, if the host holds that cell;
    /// otherwise as the root of a new domain, with no authority from the
    /// bytes (spec `snapshots`, loaded snapshots hold no authority): egress
    /// and ceiling `none`, limits within the quotas the host left it, no
    /// CPU set. Either way it is a copy and takes a new number (design
    /// D22), never the one the snapshot holds (1, its likeliest, when a
    /// version-10 file holds none). Its threads keep their identifiers.
    /// `false` if the tree has no room.
    pub(super) fn enter_restored(&mut self, cell: &mut Cell, snapshot: &Snapshot) -> bool {
        let origin = snapshot.origin();
        // A cell with a memory limit has it in its quota.
        let limited = cell.quota.memory != u64::MAX;
        if let Some(at) = self.suspended.iter().position(|&s| s == origin)
            && let Some(n) = self.tree.rekey(origin, cell.id)
        {
            (cell.pid, cell.domain) = (n.pid(), n.domain());
            // Its meter sets aside what was held for it while suspended.
            self.unsuspend(at);
            self.reshare_if_by_weight();
            return true;
        }
        // A snapshot of a cell this host still holds (a template's) makes a
        // copy of it: a sibling in its domain (design D13).
        if origin != 0 && self.tree.get(origin).is_some() {
            if limited && !self.admits_copies(origin, 1) {
                return false;
            }
            // Not `cell` itself in the closure: its address would escape,
            // and every resume would copy all of it to pass it on.
            let mut at = (0, 0);
            let entered = self.add_copies(origin, &[cell.id], limited, &mut |_, pid, domain| {
                at = (pid, domain);
            });
            (cell.pid, cell.domain) = at;
            return entered;
        }
        let Ok(spec) = snapshot.placed(&Egress::None, &Egress::None, root_limits(&cell.quota))
        else {
            return false;
        };
        let source = match snapshot.tree().pid {
            0 => 1,
            pid => pid,
        };
        let plain = super::shares::plain_root(&spec);
        let reserved = spec.limits.reservation() != Limits::ZERO;
        if reserved && !self.admits_root(cell.id, spec.limits) {
            return false;
        }
        let Ok(pid) = self.tree.add_root(cell.id, spec, source) else {
            return false;
        };
        (cell.pid, cell.domain) = (pid, cell.id);
        self.reshare_after(plain);
        if reserved {
            self.refresh_reserved();
        }
        true
    }

    /// Takes suspended cell `suspended[at]` off the list (resumed or gone),
    /// with the frames set aside for it.
    fn unsuspend(&mut self, at: usize) {
        let id = self.suspended.swap_remove(at);
        if let Some(i) = self.held_aside.iter().position(|&(c, _)| c == id) {
            memory::RESERVE.release(self.held_aside.swap_remove(i).1);
        }
    }

    /// Takes `cell`, just entered by [`enter_restored`] from a snapshot of
    /// cell `origin`, out of the tree again: suspended as it was, or gone.
    ///
    /// [`enter_restored`]: Self::enter_restored
    pub(super) fn leave_restored(&mut self, cell: &Cell, origin: u32) {
        if cell.domain == cell.id || self.tree.get(origin).is_some() {
            let _ = self.tree.end(cell.id, &mut fresh_domain); // a new root or a copy
        } else if self.tree.rekey(cell.id, origin).is_some() {
            self.suspended.push(origin);
        }
    }

    /// What cell `id` uses itself, as limits: its frames, CPU time and
    /// sockets while it is live, nothing otherwise.
    fn usage(&mut self, id: u32) -> Limits {
        if id == rhizo_cap::HOST {
            return Limits::ZERO; // not a cell (and 0 is no cell's identifier)
        }
        self.with_live(id, |live| Limits {
            memory: live.frames.used() * FRAME_SIZE,
            cpu_ns: live.cell.cpu_ns,
            sockets: live.fs.sockets().count() as u32,
            cells: 0,
        })
        .unwrap_or(Limits::ZERO)
    }

    /// Cell `id` ended (`Ending::Suspended` excepted): applies its end to
    /// the tree once the host is between operations ([`settle`]).
    ///
    /// [`settle`]: Self::settle
    pub(super) fn ended(&mut self, id: u32, ending: Ending) {
        if ending == Ending::Suspended {
            self.suspended.push(id);
        } else {
            self.ends.push(id);
        }
    }

    /// The snapshot of a suspended cell is dropped (killed): its node ends,
    /// with its subtree.
    #[cfg_attr(
        not(feature = "control"),
        allow(dead_code, reason = "used by the control kernel")
    )]
    pub fn drop_snapshot(&mut self, snapshot: &Snapshot) {
        let origin = snapshot.origin();
        if let Some(at) = self.suspended.iter().position(|&s| s == origin) {
            self.unsuspend(at);
            self.ends.push(origin);
            self.settle();
        }
    }

    /// Applies the ends of cells to the tree (design D3): the descendants
    /// that cascade end too (live ones killed, templates dropped, suspended
    /// ones reported killed in [`finished`](Self::finished)), and those
    /// promoted take their new domain and numbers.
    pub(super) fn settle(&mut self) {
        if self.settling || self.ends.is_empty() {
            return;
        }
        self.settling = true;
        while let Some(id) = self.ends.pop() {
            let parent = self.tree.get(id).map_or(HOST, |n| n.parent());
            let Ok(ended) = self.tree.end(id, &mut fresh_domain) else {
                continue;
            };
            for &gone in &ended.gone {
                forget_network(gone);
                if let Some(at) = self.suspended.iter().position(|&s| s == gone) {
                    self.unsuspend(at);
                    self.finished.push(Finished {
                        id: gone,
                        ending: Ending::Killed,
                        output: Vec::new(),
                        cpus: 0,
                        peak: 0,
                    });
                } else if !self.remove_template(gone)
                    && let Some((live, _)) = self.take_cell(gone)
                {
                    self.end(live, Ending::Killed);
                }
            }
            for &root in &ended.promoted {
                for cell in self.tree.subtree(root).unwrap_or_default() {
                    self.sync(cell);
                    // It left its domain (design D21): a cell that cannot
                    // take private copies of what it shared there ends.
                    if !self.unshare(cell) && !self.kill(cell) {
                        self.drop_template(cell);
                    }
                }
            }
            // Its parent got its reservation back, less promoted ones.
            self.sync_up(parent);
        }
        self.settling = false;
        self.release_held();
        self.reshare_if_by_weight();
        if self.reserved != Limits::ZERO {
            self.refresh_reserved();
        }
    }

    /// Gives cell `id` (live or a template) what its node says: its number,
    /// domain, and as its quotas what it may use itself (its limits less its
    /// children's reservations).
    fn sync(&mut self, id: u32) {
        let (Some(n), Some(own)) = (self.tree.get(id), self.tree.own(id)) else {
            return;
        };
        let (pid, domain) = (n.pid(), n.domain());
        let apply = |cell: &mut Cell| {
            (cell.pid, cell.domain) = (pid, domain);
            let q = &mut cell.quota;
            (q.memory, q.cpu_ns, q.sockets) = (own.memory, own.cpu_ns, own.sockets);
        };
        if let Some((cell, ..)) = self.templates.iter_mut().find(|(c, ..)| c.id == id) {
            apply(cell);
            return;
        }
        self.with_live(id, |live| {
            apply(&mut live.cell);
            let q = live.cell.quota;
            live.frames.set_limit(q.frames());
            live.fs.set_limits(q.files() as usize, q.sockets as usize);
        });
    }

    /// [`sync`](Self::sync) for cell `id` and each of its ancestors.
    fn sync_up(&mut self, mut id: u32) {
        while id != HOST {
            self.sync(id);
            id = self.tree.get(id).map_or(HOST, |n| n.parent());
        }
    }

    /// [`sync`](Self::sync) for every cell of `id`'s subtree.
    fn sync_down(&mut self, id: u32) {
        for cell in self.tree.subtree(id).unwrap_or_default() {
            self.sync(cell);
        }
    }

    /// Sets the limits of cell `id` from the host, for its memory, CPU time
    /// and sockets from `quota` and its descendants from `cells`; the cell
    /// may then use them less its children's reservations. `false` if they
    /// do not fit its parent's budget or its children's reservations.
    pub(super) fn set_limits_from(&mut self, id: u32, quota: &Quota, cells: Option<u32>) -> bool {
        let Some(n) = self.tree.get(id) else {
            return false;
        };
        let limits = Limits {
            cells: cells.unwrap_or(n.limits().cells),
            ..root_limits(quota)
        };
        // What it holds already stays (spec `isolation`): only its children
        // count against the new limits.
        let root = n.is_root();
        let used = self.usage(n.parent());
        if root && !self.admits_root(id, limits) {
            return false;
        }
        let usage = |c: u32| if c == id { Limits::ZERO } else { used };
        if self
            .tree
            .set_limits(id, limits, Actor::Host, &usage)
            .is_err()
        {
            return false;
        }
        self.sync(id);
        self.reshare_if_by_weight();
        if root {
            self.refresh_reserved();
        }
        true
    }

    /// Makes root `id`, which has no children, a child of `parent` with
    /// `spec` (the host's placement of a cell it made, spec
    /// `cell-hierarchy`): its limits are carved from the parent's free
    /// budget, its egress must be within the parent's, and it takes a
    /// number in the parent's domain. Refused, changing nothing, unless the
    /// host asks: a root is no cell's to move.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn adopt(&mut self, id: u32, parent: u32, spec: Spec, by: Actor) -> Result<(), TreeError> {
        let n = self.tree.get(id).ok_or(TreeError::NotFound)?;
        if by != Actor::Host {
            return Err(TreeError::Denied);
        }
        if self.tree.get(parent).is_none() {
            return Err(TreeError::NotFound);
        }
        if !n.is_root() {
            return Err(TreeError::Invalid);
        }
        self.place(id, parent, spec, 0)
    }

    /// Places cell `id`, which the host just made and which has no
    /// children, under `parent` (`HOST`: as the root of a new domain) with
    /// `spec` (protocol version 4: `run`, `load` and the fork of a
    /// template, design D14 of `add-cell-hierarchy`). Under a parent its
    /// limits are carved from the parent's free budget and its policies
    /// must be within the parent's; as a root, a limit `spec` leaves
    /// unlimited stays what it was. It never takes the number `skip` in
    /// its new domain (a copy's source's, design D22; 0 for none). Refused,
    /// with the cell back where it was, if the tree does not take it.
    #[cfg_attr(
        not(any(feature = "ktest", feature = "control")),
        allow(dead_code, reason = "used by in-kernel tests and the control kernel")
    )]
    pub fn place(
        &mut self,
        id: u32,
        parent: u32,
        mut spec: Spec,
        skip: u32,
    ) -> Result<(), TreeError> {
        let n = self.tree.get(id).ok_or(TreeError::NotFound)?;
        if n.children() > 0 {
            return Err(TreeError::Invalid);
        }
        if parent != HOST && self.tree.get(parent).is_none() {
            return Err(TreeError::NotFound);
        }
        // An exclusive CPU set (design D17): CPUs the machine has, never
        // the boot CPU, and none another root's set holds.
        if parent == HOST && spec.cpus != 0 && !super::shares::set_fits(&self.tree, id, spec.cpus) {
            // The control kernel audits the refusal as a tree denial.
            return Err(TreeError::Invalid);
        }
        let (saved, old, old_pid) = (n.spec(), n.parent(), n.pid());
        // Into another domain (a root's is new), it first takes private
        // copies of what it shares, or stays (design D21).
        let domain = n.domain();
        if self.tree.domain(parent) != Some(domain) {
            self.unshare_subtree(id)?;
        }
        if parent == HOST {
            let (l, was) = (&mut spec.limits, saved.limits);
            l.memory = if l.memory == u64::MAX {
                was.memory
            } else {
                l.memory
            };
            l.cpu_ns = if l.cpu_ns == u64::MAX {
                was.cpu_ns
            } else {
                l.cpu_ns
            };
            l.sockets = if l.sockets == u32::MAX {
                was.sockets
            } else {
                l.sockets
            };
        }
        let plain = parent == HOST && super::shares::plain_root(&spec);
        if parent == HOST && !self.admits_root(id, spec.limits) {
            return Err(TreeError::Budget);
        }
        let used = self.usage(parent);
        self.tree.end(id, &mut fresh_domain)?; // a leaf: allocates nothing
        let placed = if parent == HOST {
            self.tree.add_root(id, spec, skip).map(|_| ())
        } else {
            self.tree
                .add_child_unlike(id, parent, spec, Actor::Host, &|_| used, skip)
        };
        if let Err(e) = placed {
            // Back where it was, under the number it had as a root.
            let _ = if old == HOST {
                self.tree
                    .add_root(id, saved, u32::from(old_pid == 2))
                    .map(|_| ())
            } else {
                let used = self.usage(old);
                self.tree.add_child(id, old, saved, Actor::Host, &|_| used)
            };
            self.sync(id);
            self.sync_networks(id);
            return Err(e);
        }
        self.sync(id);
        self.sync_networks(id);
        self.sync_up(parent);
        self.sync_up(old);
        self.reshare_after(plain);
        self.refresh_reserved();
        // A cell placed before it ran: its thread takes its new number.
        let pid = self.tree.get(id).map_or(0, |n| n.pid());
        self.with_live(id, |live| {
            let t = &live.cell.threads;
            if t.len() == 1 && t.current().tid == old_pid && live.cell.cpu_ns == 0 {
                live.cell.renumber(pid);
            }
        });
        Ok(())
    }

    /// Sets cell `id`'s limits at `by`'s request (protocol version 4,
    /// `limits`): within its parent's free budget, and never below what it
    /// uses and its children reserved.
    #[cfg_attr(
        not(feature = "control"),
        allow(dead_code, reason = "used by the control kernel")
    )]
    pub fn set_tree_limits(&mut self, id: u32, limits: Limits, by: Actor) -> Result<(), TreeError> {
        let parent = self.tree.get(id).ok_or(TreeError::NotFound)?.parent();
        if parent == HOST && !self.admits_root(id, limits) {
            return Err(TreeError::Budget);
        }
        let (own, theirs) = (self.usage(id), self.usage(parent));
        let usage = |c: u32| if c == id { own } else { theirs };
        self.tree.set_limits(id, limits, by, &usage)?;
        self.sync(id);
        self.sync_up(parent);
        self.refresh_reserved();
        Ok(())
    }

    /// Sets cell `id`'s egress policy and ceiling at `by`'s request
    /// (protocol version 4, `policy`): see `rhizo_cap::Tree::set_egress`.
    #[cfg_attr(
        not(any(feature = "ktest", feature = "bench-cell", feature = "control")),
        allow(
            dead_code,
            reason = "used by in-kernel tests, benchmarks and the control kernel"
        )
    )]
    pub fn set_policy(
        &mut self,
        id: u32,
        egress: Egress,
        ceiling: Egress,
        by: Actor,
    ) -> Result<(), TreeError> {
        self.tree.set_egress(id, egress, ceiling, by)?;
        // Enforced from now on (design D7): narrowed connections reset.
        self.sync_networks(id);
        Ok(())
    }

    /// Moves cell `id` under `to` (`HOST`: makes it a root) at `by`'s
    /// request (spec `cell-hierarchy`, re-parenting and promotion): see
    /// `rhizo_cap::Tree::reparent`. The cells whose budget, policy, domain
    /// or number changed get them.
    #[cfg_attr(
        not(any(feature = "ktest", feature = "control")),
        allow(dead_code, reason = "used by in-kernel tests and the control kernel")
    )]
    pub fn reparent(&mut self, id: u32, to: u32, by: Actor) -> Result<(), TreeError> {
        let n = self.tree.get(id).ok_or(TreeError::NotFound)?;
        let (old, limits) = (n.parent(), n.limits());
        // The host's promotion keeps the parent's limits: the new root's
        // whole reservation is new to the kernel (a parent's moves it).
        if to == HOST && by == Actor::Host && old != HOST && !admits(limits.reservation()) {
            return Err(TreeError::Budget);
        }
        // Leaving its domain, its subtree first takes private copies of
        // what it shares there, or the move is refused (design D21).
        let leaves = match to {
            HOST => self.tree.get(id).is_some_and(|n| !n.is_root()),
            _ => self.tree.domain(to) != self.tree.domain(id),
        };
        if leaves && self.tree.controls(by, id) {
            self.unshare_subtree(id)?;
        }
        let used = self.usage(to);
        self.tree
            .reparent(id, to, by, &|_| used, &mut fresh_domain)?;
        self.sync_down(id);
        self.sync_networks(id);
        self.sync_up(old);
        self.sync_up(to);
        self.reshare();
        self.refresh_reserved();
        Ok(())
    }

    /// Ends cell `id` and its whole subtree at `by`'s request, whatever
    /// their policy for when a parent ends (spec `control-plane`, subtree
    /// kill): live cells are killed, templates dropped and suspended ones
    /// reported killed in [`finished`](Self::finished).
    #[cfg_attr(
        not(any(feature = "ktest", feature = "control")),
        allow(dead_code, reason = "used by in-kernel tests and the control kernel")
    )]
    pub fn kill_tree(&mut self, id: u32, by: Actor) -> Result<(), TreeError> {
        if !self.tree.controls(by, id) {
            return Err(self.refusal(id));
        }
        let sub = self.tree.subtree(id)?;
        for &cell in sub.iter().rev() {
            self.end_node(cell);
        }
        Ok(())
    }

    /// Suspends every live cell of `id`'s subtree at `by`'s request (spec
    /// `cell-hierarchy`: ownership is not scheduling, so only on request);
    /// returns each suspended cell's identifier and snapshot. A cell whose
    /// snapshot does not fit keeps running.
    #[cfg_attr(
        not(any(feature = "ktest", feature = "control")),
        allow(dead_code, reason = "used by in-kernel tests and the control kernel")
    )]
    pub fn suspend_tree(&mut self, id: u32, by: Actor) -> Result<Vec<(u32, Snapshot)>, TreeError> {
        if !self.tree.controls(by, id) {
            return Err(self.refusal(id));
        }
        let sub = self.tree.subtree(id)?;
        let mut out = Vec::new();
        for cell in sub {
            if let Some(snapshot) = self.suspend(cell) {
                out.push((cell, snapshot));
            }
        }
        Ok(out)
    }

    /// Why `by` was refused an operation on cell `id`.
    fn refusal(&self, id: u32) -> TreeError {
        match self.tree.get(id) {
            Some(_) => TreeError::Denied,
            None => TreeError::NotFound,
        }
    }

    /// Ends cell `id` alone, wherever it is (its children are ended first
    /// by [`kill_tree`](Self::kill_tree)).
    fn end_node(&mut self, id: u32) {
        if let Some(at) = self.suspended.iter().position(|&s| s == id) {
            self.unsuspend(at);
            self.finished.push(Finished {
                id,
                ending: Ending::Killed,
                output: Vec::new(),
                cpus: 0,
                peak: 0,
            });
            self.ends.push(id);
            self.settle();
        } else if !self.drop_template(id) {
            self.kill(id);
        }
    }
}
