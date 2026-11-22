//! How cells share the CPUs (design D23 of `add-cell-hierarchy`): domains
//! by their roots' weights, then the cells of a domain by their weights
//! flattened along the tree, both by virtual time; and CPU caps per
//! subtree, over periods of [`PERIOD_NS`].
//!
//! Domains, cells and capped nodes are plain identifiers: the tree is the
//! caller's. [`flatten`] turns a tree into what the scheduler keeps per
//! cell, when the tree changes, never when a cell runs.

use alloc::vec::Vec;
use core::ops::Range;

/// A node's CPU weight when it sets none.
pub const DEFAULT_WEIGHT: u32 = 100;

/// The period over which a CPU cap is counted: a cap of 1.5 CPUs is
/// 150 ms of CPU time per period, for the whole subtree.
pub const PERIOD_NS: u64 = 100_000_000;

/// The flattened share of a cell that has its whole domain to itself.
pub const WHOLE: u64 = 1 << 32;

/// One node of a tree of cells, as [`flatten`] reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Node {
    /// Its parent's index in the same slice; `None` for a domain's root.
    pub parent: Option<usize>,
    /// Its CPU weight (1 to 10,000).
    pub weight: u32,
    /// Its CPU cap, in nanoseconds of CPU time per [`PERIOD_NS`].
    pub cap_ns: Option<u64>,
}

/// What the scheduler keeps of a node's place in its tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// The index of its domain's root.
    pub domain: usize,
    /// Its part of its domain's CPU time, out of [`WHOLE`]: the product of
    /// its shares at each level (at least 1).
    pub share: u64,
    /// The indices of the capped nodes its CPU time counts against: itself
    /// and its ancestors that have a cap, nearest first.
    pub capped: Vec<usize>,
}

/// The place of every node of a forest of trees (indices into `nodes`,
/// parents may come in any order).
///
/// A node's part is shared by the node itself and its children, in
/// proportion to their weights: a root of weight 100 with two children of
/// weight 100 runs a third of its domain's time, as each child does.
pub fn flatten(nodes: &[Node]) -> Vec<Place> {
    let weight = |i: usize| u128::from(nodes[i].weight.max(1));
    let mut children = alloc::vec![0u128; nodes.len()];
    for n in nodes {
        if let Some(p) = n.parent {
            children[p] += u128::from(n.weight.max(1));
        }
    }
    (0..nodes.len())
        .map(|i| {
            // The path from the root down to `i` (bounded: no cycle loops).
            let mut path = alloc::vec![i];
            while let Some(p) = nodes[*path.last().unwrap()].parent {
                if path.len() > nodes.len() {
                    break;
                }
                path.push(p);
            }
            let domain = *path.last().unwrap();
            let mut part = u128::from(WHOLE);
            for pair in path.windows(2).rev() {
                let (child, parent) = (pair[0], pair[1]);
                part = part * weight(child) / (weight(parent) + children[parent]);
            }
            let share = part * weight(i) / (weight(i) + children[i]);
            let capped = path
                .iter()
                .copied()
                .filter(|&n| nodes[n].cap_ns.is_some())
                .collect();
            Place {
                domain,
                share: (share as u64).max(1),
                capped,
            }
        })
        .collect()
}

/// A cell's CPU time used, divided by its share of its domain.
#[derive(Debug, Clone, Copy)]
struct Account {
    /// `2^64 / weight`: a charge multiplies, never divides.
    inv: u64,
    vtime: u128,
    /// What a full slice adds to [`vtime`](Self::vtime).
    slice: u128,
}

impl Account {
    fn new(weight: u64) -> Self {
        let weight = weight.max(1);
        let inv = reciprocal(weight);
        Self {
            inv,
            vtime: 0,
            slice: scaled(crate::SLICE_NS, inv),
        }
    }

    fn set_weight(&mut self, weight: u64) {
        *self = Self {
            vtime: self.vtime,
            ..Self::new(weight)
        };
    }

    fn charge(&mut self, ns: u64) {
        self.vtime += scaled(ns, self.inv);
    }

    /// Its virtual time at the end of a full slice from now.
    fn finish(&self) -> u128 {
        self.vtime + self.slice
    }
}

/// A domain's CPU time used, divided by its weight, in units of 2^-16 ns
/// per unit of weight: 64 bits, compared by their wrapping difference
/// ([`before`]), as the virtual times compared stay within 2^63 units of
/// each other (39 hours of one CPU's time for a weight of 1).
#[derive(Debug, Clone, Copy)]
struct Paced {
    weight: u64,
    inv: u64,
    vtime: u64,
    /// What the longest slice charged so far ([`Fair::longest`]) adds to
    /// [`vtime`](Self::vtime).
    slice: u64,
}

impl Paced {
    fn new(weight: u64, longest: u64) -> Self {
        let weight = weight.max(1);
        let inv = reciprocal(weight);
        Self {
            weight,
            inv,
            vtime: 0,
            slice: paced(longest, inv),
        }
    }

    fn set_weight(&mut self, weight: u64, longest: u64) {
        *self = Self {
            vtime: self.vtime,
            ..Self::new(weight, longest)
        };
    }

    fn charge(&mut self, ns: u64) {
        self.vtime = self.vtime.wrapping_add(paced(ns, self.inv));
    }

    /// Its virtual time at the end of a slice as long as the longest yet,
    /// from now.
    fn finish(&self) -> u64 {
        self.vtime.wrapping_add(self.slice)
    }
}

/// `ns * 2^16 / weight` from `inv`, the weight's [`reciprocal`].
#[inline]
fn paced(ns: u64, inv: u64) -> u64 {
    ((u128::from(ns) * u128::from(inv)) >> 48) as u64
}

/// Whether virtual time `a` comes before `b`.
#[inline]
fn before(a: u64, b: u64) -> bool {
    (a.wrapping_sub(b) as i64) < 0
}

/// The later of virtual times `a` and `b`.
#[inline]
fn later(a: u64, b: u64) -> u64 {
    if before(a, b) { b } else { a }
}

/// `2^64 / weight` (at most `u64::MAX`).
fn reciprocal(weight: u64) -> u64 {
    if weight <= 1 {
        u64::MAX
    } else {
        ((1u128 << 64) / u128::from(weight)) as u64
    }
}

/// `ns * WHOLE / weight` from `inv`, the weight's [`reciprocal`]: one
/// multiplication (within one unit of the quotient).
#[inline]
fn scaled(ns: u64, inv: u64) -> u128 {
    (u128::from(ns) * u128::from(inv)) >> 32
}

/// A cell the scheduler shares by weight.
#[derive(Debug, Clone)]
struct Member {
    key: u32,
    domain: u32,
    /// Its domain's index in [`Fair::domains`] (valid while not dirty).
    dom: u32,
    account: Account,
    capped: Vec<u32>,
}

/// A queued cell of one domain on one CPU, by the virtual time it would
/// end a full slice with, then arrival.
#[derive(Debug, Clone, Copy)]
struct Slot {
    finish: u128,
    arrival: u64,
    key: u32,
    /// Its cell's position in [`Fair::cells`] (valid while not dirty).
    cell: u32,
}

impl Slot {
    fn before(&self, other: &Self) -> bool {
        (self.finish, self.arrival) < (other.finish, other.arrival)
    }
}

/// The queued cells of one domain on one CPU: a binary heap, least
/// [`Slot::finish`] first.
#[derive(Debug, Clone, Default)]
struct Lane {
    heap: Vec<Slot>,
    /// Whether its domain is in its CPU's [`Clock::present`]: from its
    /// first queued cell until a pick finds it empty, so that a domain
    /// whose cell runs and comes back does not leave and join each time.
    listed: bool,
}

impl Lane {
    #[inline]
    fn push(&mut self, slot: Slot) {
        self.heap.push(slot);
        self.up(self.heap.len() - 1);
    }

    /// Removes the slot at `i`.
    #[inline]
    fn remove(&mut self, i: usize) -> Slot {
        if self.heap.len() == 1 {
            return self.heap.pop().expect("one slot");
        }
        let slot = self.heap.swap_remove(i);
        if i < self.heap.len() {
            self.down(i);
            self.up(i);
        }
        slot
    }

    fn up(&mut self, mut i: usize) {
        while i > 0 {
            let parent = (i - 1) / 2;
            if !self.heap[i].before(&self.heap[parent]) {
                break;
            }
            self.heap.swap(i, parent);
            i = parent;
        }
    }

    fn down(&mut self, mut i: usize) {
        loop {
            let (l, r) = (2 * i + 1, 2 * i + 2);
            let mut least = i;
            if l < self.heap.len() && self.heap[l].before(&self.heap[least]) {
                least = l;
            }
            if r < self.heap.len() && self.heap[r].before(&self.heap[least]) {
                least = r;
            }
            if least == i {
                return;
            }
            self.heap.swap(i, least);
            i = least;
        }
    }
}

/// A domain: its weight and virtual time, and its queued cells on each
/// CPU.
#[derive(Debug, Clone)]
struct Dom {
    id: u32,
    account: Paced,
    lanes: Vec<Lane>,
    /// The pick that last counted it.
    stamp: u32,
    /// The CPU time charged to it, exactly (in-kernel tests).
    #[cfg(any(test, feature = "ktest"))]
    charged: u64,
}

/// A capped node: its budget per period and what it used in the current
/// one.
#[derive(Debug, Clone, Copy)]
struct Cap {
    node: u32,
    budget: u64,
    period: u64,
    used: u64,
    /// When it used its budget: the end of the period it may not run in.
    until: Option<u64>,
}

/// One CPU's queue as the scheduler shares it by weight, and the virtual
/// time of the CPU: how far a domain that always had work on it would
/// have gone.
#[derive(Debug, Clone, Default)]
struct Clock {
    vtime: u64,
    /// The weights of the domains it had work of at its last pick, and
    /// their [`reciprocal`].
    weight: u64,
    inv: u64,
    /// The domains with queued cells here (indices into `Fair::domains`),
    /// and some whose lane here has emptied since (see [`Lane::listed`]).
    present: Vec<u32>,
    /// Queued entries no cell is known for, by arrival: run first.
    strays: Vec<(u64, u32)>,
    /// The key and position in [`Fair::cells`] of the cell last picked
    /// for this CPU: what its charge and requeue look up.
    last: Option<(u32, u32)>,
}

impl Clock {
    /// Sets the weights it had work of, computing their reciprocal only
    /// when they changed.
    fn set_weight(&mut self, weight: u64) {
        if weight != self.weight {
            self.weight = weight;
            self.inv = reciprocal(weight);
        }
    }
}

/// How the entries of a [`Scheduler`](crate::Scheduler) share the CPUs.
/// `()` is round robin, at no cost: every hook does nothing and is
/// compiled away. `Option<Fair>` chooses at run time: round robin while
/// `None`, at the cost of a test in each locked section.
///
/// While entries share by weight, the scheduler tells the share every
/// entry it queues ([`queued`](Self::queued)) and every one it takes out
/// other than by [`pick`](Self::pick) ([`dequeued`](Self::dequeued)), so
/// that a pick needs no search of the queue.
pub trait Share {
    /// No weight, nothing used, for `cpus` CPUs.
    fn new(cpus: usize) -> Self;

    /// Whether a CPU picks among its queue by weight rather than taking the
    /// first entry.
    fn by_weight(&self) -> bool {
        false
    }

    /// Whether weights changed since the queues were last told
    /// ([`rebuild`](Self::rebuild) then).
    #[inline]
    fn dirty(&self) -> bool {
        false
    }

    /// Forgets what was queued and learns it again from `entries`: each
    /// queued entry's CPU, arrival number and key.
    fn rebuild(&mut self, _entries: impl Iterator<Item = (usize, u64, u32)>) {}

    /// Whether a cap holds the entry known by `key` out of the run queues,
    /// and until when.
    fn held(&self, _key: u32) -> Option<u64> {
        None
    }

    /// The entry known by `key` becomes runnable on `cpu` (it did not just
    /// run): to call before it is queued.
    fn arrive(&mut self, _cpu: usize, _key: u32) {}

    /// The entry known by `key` joined `cpu`'s queue with number `arrival`.
    fn queued(&mut self, _cpu: usize, _key: u32, _arrival: u64) {}

    /// The entry known by `key` left `cpu`'s queue other than by a pick.
    fn dequeued(&mut self, _cpu: usize, _key: u32) {}

    /// Of the queues `queues`, the one to take from for `cpu` and the
    /// arrival number of the entry to take there (only
    /// [`by_weight`](Self::by_weight)).
    fn pick(&mut self, _cpu: usize, _queues: Range<usize>) -> Option<(usize, u64)> {
        None
    }

    /// Lets the caps whose period ended by `now` run again.
    fn release(&mut self, _now: u64) {}

    /// Charges `ns` of CPU time on `cpu`, ending at `now`, to the entry
    /// known by `key`; returns whether it used up a cap
    /// ([`spent`](Self::spent) says which).
    fn charge(&mut self, _cpu: usize, _key: u32, _ns: u64, _now: u64) -> bool {
        false
    }

    /// The capped nodes whose cap the last charges used up.
    fn spent(&mut self) -> Vec<u32> {
        Vec::new()
    }

    /// Whether the entry known by `key` counts against `node`'s cap.
    fn under(&self, _key: u32, _node: u32) -> bool {
        false
    }
}

impl Share for () {
    fn new(_cpus: usize) -> Self {}
}

/// Round robin while `None`; by weight once a [`Fair`] is put in.
impl Share for Option<Fair> {
    fn new(_cpus: usize) -> Self {
        None
    }

    fn by_weight(&self) -> bool {
        self.is_some()
    }

    #[inline]
    fn dirty(&self) -> bool {
        self.as_ref().is_some_and(Fair::dirty)
    }

    fn rebuild(&mut self, entries: impl Iterator<Item = (usize, u64, u32)>) {
        if let Some(f) = self {
            f.rebuild(entries);
        }
    }

    #[inline]
    fn held(&self, key: u32) -> Option<u64> {
        self.as_ref()?.held(key)
    }

    fn arrive(&mut self, cpu: usize, key: u32) {
        if let Some(f) = self {
            f.arrive(cpu, key);
        }
    }

    #[inline]
    fn queued(&mut self, cpu: usize, key: u32, arrival: u64) {
        if let Some(f) = self {
            f.queued(cpu, key, arrival);
        }
    }

    fn dequeued(&mut self, cpu: usize, key: u32) {
        if let Some(f) = self {
            f.dequeued(cpu, key);
        }
    }

    #[inline]
    fn pick(&mut self, cpu: usize, queues: Range<usize>) -> Option<(usize, u64)> {
        self.as_mut()?.pick(cpu, queues)
    }

    #[inline]
    fn release(&mut self, now: u64) {
        if let Some(f) = self {
            f.release(now);
        }
    }

    #[inline]
    fn charge(&mut self, cpu: usize, key: u32, ns: u64, now: u64) -> bool {
        self.as_mut().is_some_and(|f| f.charge(cpu, key, ns, now))
    }

    fn spent(&mut self) -> Vec<u32> {
        self.as_mut().map_or_else(Vec::new, Share::spent)
    }

    fn under(&self, key: u32, node: u32) -> bool {
        self.as_ref().is_some_and(|f| f.under(key, node))
    }
}

/// Weights, virtual times and caps, by identifier: entries share the CPUs
/// by weight. Domains, cells and nodes are set when the tree changes,
/// never when a cell runs; each table is a vector sorted by identifier.
#[derive(Debug, Clone)]
pub struct Fair {
    domains: Vec<Dom>,
    cells: Vec<Member>,
    caps: Vec<Cap>,
    clocks: Vec<Clock>,
    /// The longest slice charged so far, on any CPU: the slice a domain's
    /// finishing virtual time counts (see [`Share::pick`]).
    longest: u64,
    /// Set by every change of the tables: the queues are learnt again.
    dirty: bool,
    spent: Vec<u32>,
    stamp: u32,
}

impl Share for Fair {
    fn new(cpus: usize) -> Self {
        Self {
            domains: Vec::new(),
            cells: Vec::new(),
            caps: Vec::new(),
            clocks: alloc::vec![Clock::default(); cpus.max(1)],
            longest: 0,
            dirty: true,
            spent: Vec::new(),
            stamp: 0,
        }
    }

    fn by_weight(&self) -> bool {
        true
    }

    #[inline]
    fn dirty(&self) -> bool {
        self.dirty
    }

    fn rebuild(&mut self, entries: impl Iterator<Item = (usize, u64, u32)>) {
        let cpus = self.clocks.len();
        for clock in &mut self.clocks {
            clock.present.clear();
            clock.strays.clear();
            clock.last = None;
        }
        for d in &mut self.domains {
            d.lanes.resize_with(cpus, Lane::default);
            for lane in &mut d.lanes {
                lane.heap.clear();
                lane.listed = false;
            }
        }
        for i in 0..self.cells.len() {
            let domain = self.cells[i].domain;
            self.cells[i].dom = self.domain_at(domain) as u32;
        }
        self.dirty = false;
        for (cpu, arrival, key) in entries {
            self.queued(cpu, key, arrival);
        }
    }

    #[inline]
    fn held(&self, key: u32) -> Option<u64> {
        if self.caps.is_empty() {
            return None;
        }
        let m = self.cell(key)?;
        m.capped.iter().filter_map(|&n| self.cap(n)?.until).max()
    }

    /// A domain or a cell that was idle on `cpu` starts at the CPU's
    /// virtual time or the least of its domain's cells there, so time it
    /// did not use goes to the others and is not owed back.
    fn arrive(&mut self, cpu: usize, key: u32) {
        let Some(i) = self.cell_at(key) else {
            return;
        };
        let dom = &mut self.domains[self.cells[i].dom as usize];
        let lane = &dom.lanes[cpu];
        if lane.heap.is_empty() {
            dom.account.vtime = later(dom.account.vtime, self.clocks[cpu].vtime);
        } else {
            let cells = &self.cells;
            let vtime = |s: &Slot| cells[s.cell as usize].account.vtime;
            let least = lane.heap.iter().map(vtime).min().unwrap_or(0);
            let account = &mut self.cells[i].account;
            account.vtime = account.vtime.max(least);
        }
    }

    #[inline(always)]
    fn queued(&mut self, cpu: usize, key: u32, arrival: u64) {
        if self.dirty {
            return; // learnt again from the queues
        }
        let Some(i) = self.cell_of(cpu, key) else {
            self.clocks[cpu].strays.push((arrival, key));
            return;
        };
        let m = &self.cells[i];
        let slot = Slot {
            finish: m.account.finish(),
            arrival,
            key,
            cell: i as u32,
        };
        let d = m.dom;
        let lane = &mut self.domains[d as usize].lanes[cpu];
        if !lane.listed {
            lane.listed = true;
            self.clocks[cpu].present.push(d);
        }
        lane.push(slot);
    }

    fn dequeued(&mut self, cpu: usize, key: u32) {
        if self.dirty {
            return;
        }
        let Some(i) = self.cell_at(key) else {
            self.clocks[cpu].strays.retain(|&(_, k)| k != key);
            return;
        };
        let d = self.cells[i].dom as usize;
        let lane = &mut self.domains[d].lanes[cpu];
        if let Some(at) = lane.heap.iter().position(|s| s.key == key) {
            lane.remove(at);
        }
    }

    /// Of the queued cells of the queues `queues`, the queue and arrival
    /// number of the one for `cpu` to run.
    ///
    /// - Domains, exactly: of those whose virtual time has not passed the
    ///   CPU's (eligible), the one that would end a full slice with the
    ///   least virtual time (worst-case fair weighted queueing, WF2Q+), so
    ///   that a domain's share never strays a slice from its weight's.
    ///   Linear in the domains with work in those queues.
    /// - Cells of that domain, approximately: the one that would end a
    ///   full slice with the least virtual time, the top of its heap.
    ///
    /// The earliest arrival on ties: round robin between equals.
    #[inline]
    fn pick(&mut self, cpu: usize, queues: Range<usize>) -> Option<(usize, u64)> {
        if queues.len() == 1 {
            let q = queues.start;
            return self.pick_in(cpu, q).map(|arrival| (q, arrival));
        }
        self.pick_across(cpu, queues)
    }

    #[inline]
    fn release(&mut self, now: u64) {
        for cap in &mut self.caps {
            if cap.until.is_some_and(|t| t <= now) {
                cap.until = None;
            }
        }
    }

    /// Charges `ns` of CPU time on `cpu`, ending at `now`, to the cell
    /// known by `key`, its domain and its capped nodes.
    #[inline]
    fn charge(&mut self, cpu: usize, key: u32, ns: u64, now: u64) -> bool {
        let Some(i) = self.cell_of(cpu, key) else {
            return false;
        };
        let clock = &mut self.clocks[cpu];
        if clock.weight != 0 {
            clock.vtime = clock.vtime.wrapping_add(paced(ns, clock.inv));
        }
        if ns > self.longest {
            self.lengthen(ns);
        }
        let m = &mut self.cells[i];
        m.account.charge(ns);
        if let Some(d) = self.domains.get_mut(m.dom as usize) {
            d.account.charge(ns);
            #[cfg(any(test, feature = "ktest"))]
            {
                d.charged += ns;
            }
        }
        if m.capped.is_empty() {
            return false;
        }
        let period = now / PERIOD_NS;
        let mut any = false;
        for &node in &m.capped {
            let Ok(c) = self.caps.binary_search_by_key(&node, |c| c.node) else {
                continue;
            };
            let cap = &mut self.caps[c];
            if cap.period != period {
                *cap = Cap {
                    period,
                    used: 0,
                    until: None,
                    ..*cap
                };
            }
            cap.used += ns;
            if cap.used >= cap.budget && cap.until.is_none() {
                cap.until = Some((period + 1) * PERIOD_NS);
                self.spent.push(node);
                any = true;
            }
        }
        any
    }

    fn spent(&mut self) -> Vec<u32> {
        core::mem::take(&mut self.spent)
    }

    /// Whether the cell known by `key` counts against `node`'s cap.
    fn under(&self, key: u32, node: u32) -> bool {
        self.cell(key).is_some_and(|m| m.capped.contains(&node))
    }
}

impl Fair {
    /// Sets the CPU weight of `domain` (1 to 10,000): domains share each
    /// CPU in proportion to their weights.
    pub fn set_domain(&mut self, domain: u32, weight: u32) {
        let at = self.domain_at(domain);
        self.domains[at]
            .account
            .set_weight(weight.into(), self.longest);
        self.dirty = true;
    }

    /// Forgets `domain`'s weight and virtual time (its last cell ended).
    pub fn forget_domain(&mut self, domain: u32) {
        if let Ok(at) = self.domains.binary_search_by_key(&domain, |d| d.id) {
            self.domains.remove(at);
            self.dirty = true;
        }
    }

    /// Shares the cell known by `key` by weight: it belongs to `domain`,
    /// runs `share` of its domain's time (out of [`WHOLE`], as [`flatten`]
    /// gives it) and counts against the caps of the `capped` nodes.
    pub fn set_cell(&mut self, key: u32, domain: u32, share: u64, capped: Vec<u32>) {
        let at = match self.cells.binary_search_by_key(&key, |m| m.key) {
            Ok(at) => at,
            Err(at) => {
                let member = Member {
                    key,
                    domain,
                    dom: 0,
                    account: Account::new(share),
                    capped: Vec::new(),
                };
                self.cells.insert(at, member);
                at
            }
        };
        let member = &mut self.cells[at];
        if member.domain != domain {
            member.account.vtime = 0;
        }
        member.domain = domain;
        member.account.set_weight(share);
        member.capped = capped;
        self.dirty = true;
    }

    /// The CPU time charged so far to `domain`, exactly (in-kernel tests
    /// compare domains by it).
    #[cfg(any(test, feature = "ktest"))]
    pub fn domain_charged_ns(&self, domain: u32) -> Option<u64> {
        let at = self.domains.binary_search_by_key(&domain, |d| d.id).ok()?;
        Some(self.domains[at].charged)
    }

    /// Forgets the cell known by `key` (it ended).
    pub fn forget_cell(&mut self, key: u32) {
        if let Some(at) = self.cell_at(key) {
            self.cells.remove(at);
            self.dirty = true;
        }
    }

    /// Forgets the cells and capped nodes whose identifier `keep` refuses
    /// (they left the tree), and the domains none of the others is in.
    pub fn retain(&mut self, keep: impl Fn(u32) -> bool) {
        self.cells.retain(|m| keep(m.key));
        self.caps.retain(|c| keep(c.node));
        let mut used: Vec<u32> = self.cells.iter().map(|m| m.domain).collect();
        used.sort_unstable();
        self.domains.retain(|d| used.binary_search(&d.id).is_ok());
        self.dirty = true;
    }

    /// Sets or removes the CPU cap of `node`: at most `cap_ns` of CPU time
    /// per [`PERIOD_NS`] for the cells that name it.
    pub fn set_cap(&mut self, node: u32, cap_ns: Option<u64>) {
        let at = self.caps.binary_search_by_key(&node, |c| c.node);
        match (cap_ns, at) {
            (Some(budget), Ok(at)) => self.caps[at].budget = budget,
            (Some(budget), Err(at)) => self.caps.insert(
                at,
                Cap {
                    node,
                    budget,
                    period: 0,
                    used: 0,
                    until: None,
                },
            ),
            (None, Ok(at)) => {
                self.caps.remove(at);
            }
            (None, Err(_)) => {}
        }
        self.dirty = true;
    }

    /// [`pick`](Share::pick) from `cpu`'s own queue or one other queue
    /// `q`: the domains with work there, one pass.
    #[inline]
    fn pick_in(&mut self, cpu: usize, q: usize) -> Option<u64> {
        let here = &mut self.clocks[q];
        if !here.strays.is_empty() {
            return Some(self.stray(q));
        }
        let d = match *here.present.as_slice() {
            [] => return None,
            // One domain with work: it runs, and the clock catches up.
            [d] if !self.domains[d as usize].lanes[q].heap.is_empty() => {
                let a = self.domains[d as usize].account;
                let clock = &mut self.clocks[cpu];
                clock.set_weight(a.weight);
                clock.vtime = later(clock.vtime, a.vtime);
                d as usize
            }
            _ => self.choose(cpu, q)?,
        };
        let slot = self.domains[d].lanes[q].remove(0);
        self.clocks[cpu].last = Some((slot.key, slot.cell));
        Some(slot.arrival)
    }

    /// The domain to run of the several with work in queue `q`, by
    /// `cpu`'s virtual time (and the domains whose lane there emptied
    /// leave its list).
    fn choose(&mut self, cpu: usize, q: usize) -> Option<usize> {
        let now = self.clocks[cpu].vtime;
        let mut weight = 0;
        // The eligible domain that would finish a slice first.
        let mut best = None::<(u64, u32)>;
        let mut i = 0;
        while i < self.clocks[q].present.len() {
            let d = self.clocks[q].present[i];
            let dom = &mut self.domains[d as usize];
            if dom.lanes[q].heap.is_empty() {
                dom.lanes[q].listed = false;
                self.clocks[q].present.swap_remove(i);
                continue;
            }
            i += 1;
            let a = &dom.account;
            weight += a.weight;
            let finish = a.finish();
            if !before(now, a.vtime) && best.is_none_or(|(f, _)| before(finish, f)) {
                best = Some((finish, d));
            }
        }
        self.clocks[cpu].set_weight(weight);
        match best {
            Some((_, d)) => Some(d as usize),
            None => self.least(cpu, q..q + 1),
        }
    }

    /// Of the domains with work in queues `queues`, none eligible, the one
    /// with the least virtual time (then finishing first, then first
    /// listed): `cpu`'s clock moves to its virtual time.
    #[cold]
    #[inline(never)]
    fn least(&mut self, cpu: usize, queues: Range<usize>) -> Option<usize> {
        let mut low = None::<(u64, u64, u32)>;
        for q in queues {
            for &d in &self.clocks[q].present {
                let dom = &self.domains[d as usize];
                if dom.lanes[q].heap.is_empty() {
                    continue;
                }
                let a = &dom.account;
                let finish = a.finish();
                let lower = |(v, f, _): (u64, u64, u32)| {
                    before(a.vtime, v) || (a.vtime == v && before(finish, f))
                };
                if low.is_none_or(lower) {
                    low = Some((a.vtime, finish, d));
                }
            }
        }
        let (vtime, _, d) = low?;
        self.clocks[cpu].vtime = vtime;
        Some(d as usize)
    }

    /// [`pick`](Share::pick) over several queues (one CPU stepping
    /// through every queue): a domain with work in several counts once.
    #[inline(never)]
    fn pick_across(&mut self, cpu: usize, queues: Range<usize>) -> Option<(usize, u64)> {
        for q in queues.clone() {
            if !self.clocks[q].strays.is_empty() {
                return Some((q, self.stray(q)));
            }
        }
        self.stamp = self.stamp.wrapping_add(1);
        let now = self.clocks[cpu].vtime;
        let mut weight = 0;
        let mut best = None::<(u64, u32)>;
        for q in queues.clone() {
            for &d in &self.clocks[q].present {
                let dom = &mut self.domains[d as usize];
                if dom.stamp == self.stamp || dom.lanes[q].heap.is_empty() {
                    continue;
                }
                dom.stamp = self.stamp;
                let a = dom.account;
                weight += a.weight;
                let finish = a.finish();
                if !before(now, a.vtime) && best.is_none_or(|(f, _)| before(finish, f)) {
                    best = Some((finish, d));
                }
            }
        }
        self.clocks[cpu].set_weight(weight);
        let d = match best {
            Some((_, d)) => d as usize,
            None => self.least(cpu, queues.clone())?,
        };
        let lanes = &self.domains[d].lanes;
        let q = queues
            .filter(|&q| !lanes[q].heap.is_empty())
            .min_by_key(|&q| (lanes[q].heap[0].finish, lanes[q].heap[0].arrival))?;
        let slot = self.domains[d].lanes[q].remove(0);
        self.clocks[cpu].last = Some((slot.key, slot.cell));
        Some((q, slot.arrival))
    }

    /// A slice of `ns`, longer than any charged before: the domains'
    /// finishing virtual times count it from now on.
    #[cold]
    #[inline(never)]
    fn lengthen(&mut self, ns: u64) {
        self.longest = ns;
        for d in &mut self.domains {
            d.account.slice = paced(ns, d.account.inv);
        }
    }

    /// Takes the first entry of queue `q` no cell is known for.
    #[cold]
    fn stray(&mut self, q: usize) -> u64 {
        self.clocks[q].strays.remove(0).0
    }

    /// The position of the cell known by `key`, the one last picked for
    /// `cpu` without a search.
    #[inline]
    fn cell_of(&self, cpu: usize, key: u32) -> Option<usize> {
        match self.clocks[cpu].last {
            Some((k, i)) if k == key => Some(i as usize),
            _ => self.cell_at(key),
        }
    }

    /// The position of the cell known by `key`.
    fn cell_at(&self, key: u32) -> Option<usize> {
        self.cells.binary_search_by_key(&key, |m| m.key).ok()
    }

    fn cell(&self, key: u32) -> Option<&Member> {
        self.cell_at(key).map(|i| &self.cells[i])
    }

    fn cap(&self, node: u32) -> Option<&Cap> {
        let at = self.caps.binary_search_by_key(&node, |c| c.node).ok()?;
        Some(&self.caps[at])
    }

    /// The position of `domain`, added with the default weight if new.
    fn domain_at(&mut self, domain: u32) -> usize {
        match self.domains.binary_search_by_key(&domain, |d| d.id) {
            Ok(at) => at,
            Err(at) => {
                let dom = Dom {
                    id: domain,
                    account: Paced::new(DEFAULT_WEIGHT.into(), self.longest),
                    lanes: alloc::vec![Lane::default(); self.clocks.len()],
                    stamp: 0,
                    #[cfg(any(test, feature = "ktest"))]
                    charged: 0,
                };
                self.domains.insert(at, dom);
                self.dirty = true;
                at
            }
        }
    }
}

#[cfg(test)]
mod fair_shares {
    use super::*;
    use crate::{SLICE_NS, Scheduler};
    use alloc::collections::BTreeMap;
    use proptest::prelude::*;

    /// Runs `slices` slices on CPU 0 from `now` (each `len(key)` long; when
    /// nothing is runnable, time jumps to the next wake-up) and returns the
    /// CPU time each key got.
    fn run(
        s: &mut Scheduler<u32, Fair>,
        slices: usize,
        now: &mut u64,
        len: impl Fn(u32) -> u64,
    ) -> BTreeMap<u32, u64> {
        let mut got = BTreeMap::new();
        for _ in 0..slices {
            let Some(key) = s.next(0, *now) else {
                *now = s.next_wake().expect("nothing runnable nor asleep");
                continue;
            };
            let ns = len(key);
            *now += ns;
            s.charge(0, key, ns, *now);
            *got.entry(key).or_insert(0) += ns;
            s.requeue(0, key);
        }
        got
    }

    /// Domain `d`'s cells are keys `d * 100 + c`, sharing it equally.
    fn domains(s: &mut Scheduler<u32, Fair>, weights: &[u32], cells: &[u32]) {
        for (d, (&w, &n)) in weights.iter().zip(cells).enumerate() {
            let d = d as u32;
            s.shares().set_domain(d, w);
            for c in 0..n {
                s.shares()
                    .set_cell(d * 100 + c, d, WHOLE / u64::from(n), Vec::new());
                s.place(d * 100 + c);
            }
        }
    }

    fn per_domain(got: &BTreeMap<u32, u64>, domains: usize) -> Vec<u64> {
        let mut slices = alloc::vec![0; domains];
        for (&key, &ns) in got {
            slices[key as usize / 100] += ns / SLICE_NS;
        }
        slices
    }

    /// Whether every domain's slices are within `bound` slices of its
    /// weight's part of `total`.
    fn within(slices: &[u64], weights: &[u32], total: u64, bound: u64) -> bool {
        let sum: u64 = weights.iter().map(|&w| u64::from(w)).sum();
        slices
            .iter()
            .zip(weights)
            .all(|(&n, &w)| (n * sum).abs_diff(total * u64::from(w)) <= bound * sum)
    }

    fn within_one_slice(slices: &[u64], weights: &[u32], total: u64) -> bool {
        within(slices, weights, total, 1)
    }

    /// Busy domains of `weights`, one cell each, run slices of lengths
    /// `lens`, each charged as it lasted, with one CPU stepping through
    /// the queues of `cpus` (as `Host::step` does): after every slice,
    /// each domain's CPU time is within the longest slice yet of its
    /// weight's part of the total, and is what
    /// [`Fair::domain_charged_ns`] reads.
    fn stretched(cpus: usize, weights: &[u32], lens: &[u64]) -> Result<(), TestCaseError> {
        let mut s = Scheduler::fair(cpus);
        domains(&mut s, weights, &alloc::vec![1; weights.len()]);
        let sum: u128 = weights.iter().map(|&w| u128::from(w)).sum();
        let mut used = alloc::vec![0u64; weights.len()];
        let (mut total, mut longest) = (0u128, 0u128);
        for &ns in lens {
            let key = s.next_oldest(0).expect("all busy");
            s.charge(0, key, ns, 0);
            s.requeue(0, key);
            let d = key as usize / 100;
            used[d] += ns;
            prop_assert_eq!(s.shares().domain_charged_ns(d as u32), Some(used[d]));
            total += u128::from(ns);
            longest = longest.max(u128::from(ns));
            for (&got, &w) in used.iter().zip(weights) {
                let off = (u128::from(got) * sum).abs_diff(total * u128::from(w));
                prop_assert!(
                    off <= longest * sum,
                    "weight {} off by {} ns, longest slice {}: {:?}",
                    w,
                    off / sum,
                    longest,
                    used
                );
            }
        }
        Ok(())
    }

    /// Slice lengths for [`stretched`]: any up to 20 nominal slices, or
    /// short ones, so that a slice longer than every one before it comes
    /// after many shorter ones.
    fn lens() -> impl Strategy<Value = Vec<u64>> {
        let len = prop_oneof![1..=20 * SLICE_NS, 1..=SLICE_NS / 50];
        proptest::collection::vec(len, 1..300)
    }

    #[test]
    fn many_cells_do_not_buy_cpu() {
        let mut s = Scheduler::fair(1);
        domains(&mut s, &[100, 100], &[16, 1]);
        let got = run(&mut s, 200, &mut 0, |_| SLICE_NS);
        assert_eq!(per_domain(&got, 2), [100, 100]);
    }

    #[test]
    fn flattened_shares_split_each_part_by_weight() {
        // 0 (100) has children 1 (100, capped) and 2 (200); 1 has child 3
        // (100, capped); 4 is another domain's root.
        let node = |parent, weight, cap_ns| Node {
            parent,
            weight,
            cap_ns,
        };
        let places = flatten(&[
            node(None, 100, None),
            node(Some(0), 100, Some(5)),
            node(Some(0), 200, None),
            node(Some(1), 100, Some(7)),
            node(None, 1, None),
        ]);
        let shares: Vec<u64> = places.iter().map(|p| p.share).collect();
        assert_eq!(
            shares,
            [WHOLE / 4, WHOLE / 8, WHOLE / 2, WHOLE / 8, WHOLE],
            "a quarter for 0 and for 1's subtree, half for 2"
        );
        let domains: Vec<usize> = places.iter().map(|p| p.domain).collect();
        assert_eq!(domains, [0, 0, 0, 0, 4]);
        assert_eq!(places[3].capped, [3, 1], "nearest first");
        assert!(places[2].capped.is_empty());
    }

    #[test]
    fn one_cpu_stepping_through_every_queue_shares_by_weight() {
        let mut s = Scheduler::fair(2);
        domains(&mut s, &[300, 100], &[1, 1]);
        assert_eq!(s.runnable(), 2);
        let mut slices = [0; 2];
        for _ in 0..400 {
            let key = s.next_oldest(0).unwrap();
            s.charge(0, key, SLICE_NS, 0);
            slices[key as usize / 100] += 1;
            s.requeue(0, key);
        }
        assert!(within_one_slice(&slices, &[300, 100], 400), "{slices:?}");
    }

    #[test]
    fn equal_shares_are_round_robin() {
        let mut s = Scheduler::fair(1);
        s.shares().set_domain(0, DEFAULT_WEIGHT);
        for key in [1, 2, 3] {
            s.shares().set_cell(key, 0, WHOLE / 3, Vec::new());
            s.place(key);
        }
        let order: Vec<u32> = (0..6)
            .map(|_| {
                let k = s.next(0, 0).unwrap();
                s.charge(0, k, SLICE_NS, 0);
                s.requeue(0, k);
                k
            })
            .collect();
        assert_eq!(order, [1, 2, 3, 1, 2, 3]);
    }

    #[test]
    fn a_switchable_scheduler_is_round_robin_until_a_fair_is_put_in() {
        let mut s = Scheduler::switchable(1);
        for key in [1, 2, 101] {
            s.place(key);
        }
        let turns = |s: &mut Scheduler<u32, Option<Fair>>, n| -> Vec<u32> {
            (0..n)
                .map(|_| {
                    let k = s.next(0, 0).unwrap();
                    s.requeue_charged(0, k, || (SLICE_NS, 0));
                    k
                })
                .collect()
        };
        assert_eq!(turns(&mut s, 6), [1, 2, 101, 1, 2, 101], "round robin");
        // Domain 0 holds cells 1 and 2, domain 1 cell 101: by weight, 101
        // runs every other slice.
        let fair = s.shares().insert(Fair::new(1));
        for (key, domain, share) in [(1, 0, WHOLE / 2), (2, 0, WHOLE / 2), (101, 1, WHOLE)] {
            fair.set_domain(domain, DEFAULT_WEIGHT);
            fair.set_cell(key, domain, share, Vec::new());
        }
        let got = turns(&mut s, 40);
        let other = got.iter().filter(|&&k| k == 101).count();
        assert_eq!(other, 20, "{got:?}");
        *s.shares() = None;
        let got = turns(&mut s, 30);
        assert_eq!(got.iter().filter(|&&k| k == 101).count(), 10, "{got:?}");
    }

    #[test]
    fn requeue_charged_holds_a_subtree_past_its_cap() {
        let mut s = Scheduler::switchable(1);
        let fair = s.shares().insert(Fair::new(1));
        fair.set_domain(0, DEFAULT_WEIGHT);
        fair.set_cap(7, Some(SLICE_NS));
        fair.set_cell(1, 0, WHOLE, alloc::vec![7]);
        s.place(1);
        let k = s.next(0, 0).unwrap();
        let mut asked = false;
        s.requeue_charged(0, k, || {
            asked = true;
            (SLICE_NS, SLICE_NS)
        });
        assert!(asked, "the slice is asked while sharing by weight");
        assert_eq!(s.runnable(), 0, "held until the period ends");
        assert_eq!(s.next_wake(), Some(PERIOD_NS));
        assert_eq!(s.next(0, PERIOD_NS), Some(1), "back at the next period");
        *s.shares() = None;
        s.requeue_charged(0, 1, || unreachable!("not asked while round robin"));
        assert_eq!(s.runnable(), 1);
    }

    #[test]
    fn retain_forgets_cells_caps_and_empty_domains() {
        let mut s = Scheduler::fair(1);
        domains(&mut s, &[100, 100], &[2, 1]);
        s.shares().set_cap(0, Some(PERIOD_NS / 10));
        s.shares().set_cell(0, 0, WHOLE / 2, alloc::vec![0]);
        s.shares().retain(|key| key != 0 && key != 100);
        assert!(s.shares().cell(1).is_some());
        assert!(s.shares().cell(0).is_none());
        assert!(s.shares().caps.is_empty(), "node 0 left");
        let domains: Vec<u32> = s.shares().domains.iter().map(|d| d.id).collect();
        assert_eq!(domains, [0], "domain 1 has no cell left");
    }

    /// What one step of [`lanes_follow_the_queues`] does.
    #[derive(Debug, Clone)]
    enum Op {
        Take(usize),
        Preempt(usize, u64),
        Sleep(usize, u64),
        Tick(u64),
        Host(u32),
        /// Entry `.0` moves to domain `.1` (a tree change).
        Move(u32, u32),
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            (0usize..4).prop_map(Op::Take),
            (0usize..4, 1..=SLICE_NS).prop_map(|(c, n)| Op::Preempt(c, n)),
            (0usize..4, 1u64..20).prop_map(|(c, d)| Op::Sleep(c, d)),
            (1u64..10).prop_map(Op::Tick),
            (0u32..12).prop_map(Op::Host),
            (0u32..12, 0u32..3).prop_map(|(k, d)| Op::Move(k, d)),
        ]
    }

    proptest! {
        /// The per-CPU heaps follow the queues through every interleaving
        /// of takes, preemptions, sleeps, removals by key and tree changes:
        /// no entry is lost or held twice, and a CPU finds nothing only
        /// when no queue holds anything and no sleeper is due.
        #[test]
        fn lanes_follow_the_queues(
            cpus in 1usize..5,
            entries in 1u32..12,
            ops in proptest::collection::vec(op(), 1..300),
        ) {
            let mut s = Scheduler::fair(cpus);
            for e in 0..entries {
                s.shares().set_domain(e % 3, 100 * (e % 3 + 1));
                s.shares().set_cell(e, e % 3, WHOLE / 4, Vec::new());
                s.place(e);
            }
            let mut running: Vec<Option<u32>> = alloc::vec![None; cpus];
            let mut now = 0;
            for op in ops {
                match op {
                    Op::Take(c) if c < cpus && running[c].is_none() => {
                        let due = s.next_wake().is_some_and(|t| t <= now);
                        let work = s.runnable() > 0 || due;
                        running[c] = s.next(c, now);
                        prop_assert_eq!(running[c].is_some(), work);
                    }
                    Op::Preempt(c, ns) if c < cpus => {
                        if let Some(e) = running[c].take() {
                            s.requeue_charged(c, e, || (ns, now));
                        }
                    }
                    Op::Sleep(c, d) if c < cpus => {
                        if let Some(e) = running[c].take() {
                            s.sleep(e, now + d);
                        }
                    }
                    Op::Tick(d) => now += d,
                    Op::Host(e) if e < entries && !running.contains(&Some(e)) => {
                        let (got, wake) = s.remove(e).expect("in the scheduler");
                        prop_assert_eq!(got, e);
                        match wake {
                            Some(t) => s.sleep(e, t),
                            None => {
                                s.place(e);
                            }
                        }
                    }
                    Op::Move(e, d) if e < entries => {
                        s.shares().set_domain(d, 100);
                        s.shares().set_cell(e, d, WHOLE / 2, Vec::new());
                    }
                    _ => {}
                }
            }
            let held = running.iter().flatten().count();
            prop_assert_eq!(s.len() + held, entries as usize);
            let mut seen = alloc::collections::BTreeSet::new();
            let end = now + 100;
            for c in 0..cpus {
                while let Some(e) = s.next(c, end) {
                    prop_assert!(seen.insert(e), "{} twice", e);
                }
            }
            prop_assert_eq!(seen.len() + held, entries as usize);
        }

        /// Busy domains share 1,000 slices by their weights within one
        /// slice, whatever their numbers of cells.
        #[test]
        fn domains_get_their_weights(
            domains_cells in proptest::collection::vec((1u32..=10_000, 1u32..=16), 2..6),
        ) {
            let (weights, cells): (Vec<u32>, Vec<u32>) = domains_cells.into_iter().unzip();
            let mut s = Scheduler::fair(1);
            domains(&mut s, &weights, &cells);
            let got = run(&mut s, 1_000, &mut 0, |_| SLICE_NS);
            let slices = per_domain(&got, weights.len());
            prop_assert!(within_one_slice(&slices, &weights, 1_000), "{:?}", slices);
        }

        /// Two busy domains, one cell each, with slices of any length, as a
        /// host that stops the CPU in the middle of one stretches them (a
        /// busy host under TCG, a descheduled vCPU), each charged as it
        /// lasted, and one CPU stepping through the queues of several (as
        /// `Host::step` does): after every slice, each domain's CPU time is
        /// within the longest slice yet of its weight's part of the total
        /// (design D23: within one slice), and is what
        /// [`Fair::domain_charged_ns`] reads.
        #[test]
        fn stretched_slices_stay_within_one_slice(
            cpus in 1usize..5,
            weights in (1u32..=10_000, 1u32..=10_000),
            lens in proptest::collection::vec(1..=20 * SLICE_NS, 1..300),
        ) {
            stretched(cpus, &[weights.0, weights.1], &lens)?;
        }

        /// The same with three to six domains, and slices of any length
        /// among short ones (change `fix-fair-bound`): within the longest
        /// slice yet, whatever the number of domains.
        #[test]
        fn many_domains_stay_within_one_stretched_slice(
            cpus in 1usize..5,
            weights in proptest::collection::vec(1u32..=10_000, 3..=6),
            lens in lens(),
        ) {
            stretched(cpus, &weights, &lens)?;
        }

        /// The cells of one domain share it by their flattened weights,
        /// approximately (design D23: exact between domains only): within
        /// two slices.
        #[test]
        fn cells_get_their_shares(weights in proptest::collection::vec(1u32..=10_000, 2..6)) {
            let mut s = Scheduler::fair(1);
            s.shares().set_domain(0, DEFAULT_WEIGHT);
            let sum: u64 = weights.iter().map(|&w| u64::from(w)).sum();
            for (c, &w) in weights.iter().enumerate() {
                s.shares().set_cell(c as u32, 0, WHOLE * u64::from(w) / sum, Vec::new());
                s.place(c as u32);
            }
            let got = run(&mut s, 1_000, &mut 0, |_| SLICE_NS);
            let slices: Vec<u64> = (0..weights.len() as u32)
                .map(|c| got.get(&c).copied().unwrap_or(0) / SLICE_NS)
                .collect();
            prop_assert!(within(&slices, &weights, 1_000, 2), "{:?}", slices);
        }

        /// While a domain is idle the busy ones share by their weights
        /// within one slice; once it is back it does not take back the time
        /// it did not use (it gets no more than its weight's part and one
        /// slice), and the others' lag from before is under a slice more.
        #[test]
        fn an_idle_domains_share_goes_to_the_others(
            weights in proptest::collection::vec(1u32..=10_000, 3..6),
            idle in 0usize..3,
        ) {
            let mut s = Scheduler::fair(1);
            let cells = alloc::vec![1; weights.len()];
            domains(&mut s, &weights, &cells);
            let idle_key = idle as u32 * 100;
            s.remove(idle_key);
            let mut now = 0;
            let got = run(&mut s, 1_000, &mut now, |_| SLICE_NS);
            let mut busy = weights.clone();
            busy[idle] = 0;
            let slices = per_domain(&got, weights.len());
            prop_assert!(within_one_slice(&slices, &busy, 1_000), "while idle: {:?}", slices);
            s.place(idle_key);
            let got = run(&mut s, 1_000, &mut now, |_| SLICE_NS);
            let slices = per_domain(&got, weights.len());
            let sum: u64 = weights.iter().map(|&w| u64::from(w)).sum();
            let back = slices[idle] * sum;
            prop_assert!(back <= 1_000 * u64::from(weights[idle]) + sum,
                "took back: {:?}", slices);
            prop_assert!(within(&slices, &weights, 1_000, 2), "after: {:?}", slices);
        }

        /// A capped subtree next to a busy domain never runs more than its
        /// cap plus one slice in a period, and the busy domain gets the
        /// rest.
        #[test]
        fn caps_hold(
            cap_ms in 1u64..100,
            cells in 1u32..5,
            lens in proptest::collection::vec(1u64..=SLICE_NS, 1..8),
        ) {
            let mut s = Scheduler::fair(1);
            domains(&mut s, &[100, 100], &[cells, 1]);
            // Node 1000, domain 0's root, is capped; its cells name it.
            s.shares().set_cap(1000, Some(cap_ms * 1_000_000));
            for c in 0..cells {
                s.shares().set_cell(c, 0, WHOLE / u64::from(cells), alloc::vec![1000]);
            }
            let mut now = 0;
            let mut capped = BTreeMap::<u64, u64>::new();
            let mut other = BTreeMap::<u64, u64>::new();
            for step in 0..2_000usize {
                let Some(key) = s.next(0, now) else {
                    now = s.next_wake().unwrap();
                    continue;
                };
                let ns = lens[step % lens.len()];
                let period = now / PERIOD_NS;
                now += ns;
                s.charge(0, key, ns, now);
                s.requeue(0, key);
                let into = if key < 100 { &mut capped } else { &mut other };
                *into.entry(period).or_insert(0) += ns;
            }
            for (&period, &ns) in &capped {
                prop_assert!(ns <= cap_ms * 1_000_000 + SLICE_NS,
                    "period {}: {} ns", period, ns);
            }
            for period in 0..now / PERIOD_NS {
                let total = capped.get(&period).unwrap_or(&0) + other.get(&period).unwrap_or(&0);
                prop_assert!(total + 2 * SLICE_NS >= PERIOD_NS, "period {} idle", period);
            }
        }
    }
}
