//! How cells share the CPUs (design D23 of `add-cell-hierarchy`): domains
//! by their roots' weights, then the cells of a domain by their weights
//! flattened along the tree, both by virtual time; and CPU caps per
//! subtree, over periods of [`PERIOD_NS`].
//!
//! Domains, cells and capped nodes are plain identifiers: the tree is the
//! caller's. [`flatten`] turns a tree into what the scheduler keeps per
//! cell, when the tree changes, never when a cell runs.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

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

/// CPU time used, divided by a weight.
#[derive(Debug, Clone, Copy)]
struct Account {
    weight: u64,
    vtime: u128,
}

impl Account {
    fn new(weight: u64) -> Self {
        Self {
            weight: weight.max(1),
            vtime: 0,
        }
    }

    fn charge(&mut self, ns: u64) {
        self.vtime += Self::scaled(ns, self.weight);
    }

    fn scaled(ns: u64, weight: u64) -> u128 {
        u128::from(ns) * u128::from(WHOLE) / u128::from(weight)
    }

    /// Its virtual time at the end of a full slice from now.
    fn finish(&self) -> u128 {
        self.vtime + Self::scaled(crate::SLICE_NS, self.weight)
    }
}

/// A cell the scheduler shares by weight.
#[derive(Debug, Clone)]
struct Member {
    domain: u32,
    account: Account,
    capped: Vec<u32>,
}

/// A capped node: its budget per period and what it used in the current
/// one.
#[derive(Debug, Clone, Copy)]
struct Cap {
    budget: u64,
    period: u64,
    used: u64,
    /// When it used its budget: the end of the period it may not run in.
    until: Option<u64>,
}

/// The virtual time of one CPU: how far a domain that always had work on
/// it would have gone.
#[derive(Debug, Clone, Copy, Default)]
struct Clock {
    vtime: u128,
    /// The weights of the domains it had work of at its last pick.
    weight: u64,
}

/// How the entries of a [`Scheduler`](crate::Scheduler) share the CPUs.
/// `()` is round robin, at no cost: every hook does nothing and is
/// compiled away. `Option<Fair>` chooses at run time: round robin while
/// `None`, at the cost of a test in each hook.
pub trait Share {
    /// No weight, nothing used, for `cpus` CPUs.
    fn new(cpus: usize) -> Self;

    /// Whether a CPU picks among its queue by weight rather than taking the
    /// first entry.
    fn by_weight(&self) -> bool {
        false
    }

    /// Whether a cap holds the entry known by `key` out of the run queues,
    /// and until when.
    fn held(&self, _key: u32) -> Option<u64> {
        None
    }

    /// The entry known by `key` becomes runnable on `cpu`, next to `peers`.
    fn arrive(&mut self, _cpu: usize, _key: u32, _peers: impl Iterator<Item = u32>) {}

    /// Of the runnable entries `keys`, the position of the one for `cpu` to
    /// run (only [`by_weight`](Self::by_weight)).
    fn pick(&mut self, _cpu: usize, _keys: impl Iterator<Item = u32> + Clone) -> usize {
        0
    }

    /// Lets the caps whose period ended by `now` run again.
    fn release(&mut self, _now: u64) {}

    /// Charges `ns` of CPU time on `cpu`, ending at `now`, to the entry
    /// known by `key`; returns the capped nodes whose cap it used up.
    fn charge(&mut self, _cpu: usize, _key: u32, _ns: u64, _now: u64) -> Vec<u32> {
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

    fn held(&self, key: u32) -> Option<u64> {
        self.as_ref()?.held(key)
    }

    fn arrive(&mut self, cpu: usize, key: u32, peers: impl Iterator<Item = u32>) {
        if let Some(f) = self {
            f.arrive(cpu, key, peers);
        }
    }

    fn pick(&mut self, cpu: usize, keys: impl Iterator<Item = u32> + Clone) -> usize {
        self.as_mut().map_or(0, |f| f.pick(cpu, keys))
    }

    fn release(&mut self, now: u64) {
        if let Some(f) = self {
            f.release(now);
        }
    }

    fn charge(&mut self, cpu: usize, key: u32, ns: u64, now: u64) -> Vec<u32> {
        self.as_mut()
            .map_or_else(Vec::new, |f| f.charge(cpu, key, ns, now))
    }

    fn under(&self, key: u32, node: u32) -> bool {
        self.as_ref().is_some_and(|f| f.under(key, node))
    }
}

/// Weights, virtual times and caps, by identifier: entries share the CPUs
/// by weight. Domains, cells and nodes are set when the tree changes,
/// never when a cell runs.
#[derive(Debug, Clone)]
pub struct Fair {
    domains: BTreeMap<u32, Account>,
    cells: BTreeMap<u32, Member>,
    caps: BTreeMap<u32, Cap>,
    clocks: Vec<Clock>,
}

impl Share for Fair {
    fn new(cpus: usize) -> Self {
        Self {
            domains: BTreeMap::new(),
            cells: BTreeMap::new(),
            caps: BTreeMap::new(),
            clocks: alloc::vec![Clock::default(); cpus],
        }
    }

    fn by_weight(&self) -> bool {
        true
    }

    fn held(&self, key: u32) -> Option<u64> {
        if self.caps.is_empty() {
            return None;
        }
        let m = self.cells.get(&key)?;
        m.capped
            .iter()
            .filter_map(|n| self.caps.get(n)?.until)
            .max()
    }

    /// Of the runnable cells `keys` of `cpu`'s queue, the position of the
    /// one to run.
    ///
    /// - Domains, exactly: of those whose virtual time has not passed the
    ///   CPU's (eligible), the one that would end a full slice with the
    ///   least virtual time (worst-case fair weighted queueing, WF2Q+), so
    ///   that a domain's share never strays a slice from its weight's.
    /// - Cells of that domain, approximately: the one that would end a
    ///   full slice with the least virtual time.
    ///
    /// The first in order on ties: round robin between equals.
    fn pick(&mut self, cpu: usize, keys: impl Iterator<Item = u32> + Clone) -> usize {
        // Each domain with work here, with its account, once.
        let mut here: Vec<(u32, Account)> = Vec::new();
        for key in keys.clone() {
            let (d, account, _) = self.accounts(key);
            if !here.iter().any(|&(h, _)| h == d) {
                here.push((d, account.copied().unwrap_or(Account::new(1))));
            }
        }
        let clock = &mut self.clocks[cpu];
        clock.weight = here.iter().map(|(_, a)| a.weight).sum();
        let least = here.iter().map(|(_, a)| a.vtime).min().unwrap_or(0);
        clock.vtime = clock.vtime.max(least);
        let now = clock.vtime;
        let domain = here
            .iter()
            .filter(|(_, a)| a.vtime <= now)
            .min_by_key(|(_, a)| a.finish())
            .map_or(u32::MAX, |&(d, _)| d);
        keys.enumerate()
            .filter_map(|(i, k)| match self.accounts(k) {
                (d, _, cell) if d == domain => Some((cell.map_or(0, Account::finish), i)),
                _ => None,
            })
            .min()
            .map_or(0, |(_, i)| i)
    }

    /// The cell known by `key` becomes runnable on `cpu`, next to `peers`:
    /// a domain or a cell that was idle starts at the CPU's virtual time or
    /// the least of its domain's cells there, so time it did not use goes
    /// to the others and is not owed back.
    fn arrive(&mut self, cpu: usize, key: u32, peers: impl Iterator<Item = u32>) {
        let Some(&Member { domain, .. }) = self.cells.get(&key) else {
            return;
        };
        let (mut busy, mut least_cell) = (false, None::<u128>);
        for peer in peers {
            if let (d, _, Some(cell)) = self.accounts(peer)
                && d == domain
            {
                busy = true;
                least_cell = Some(least_cell.map_or(cell.vtime, |t| t.min(cell.vtime)));
            }
        }
        let now = self.clocks[cpu].vtime;
        if let (false, Some(a)) = (busy, self.domains.get_mut(&domain)) {
            a.vtime = a.vtime.max(now);
        }
        if let (Some(t), Some(m)) = (least_cell, self.cells.get_mut(&key)) {
            m.account.vtime = m.account.vtime.max(t);
        }
    }

    fn release(&mut self, now: u64) {
        for cap in self.caps.values_mut() {
            if cap.until.is_some_and(|t| t <= now) {
                cap.until = None;
            }
        }
    }

    /// Charges `ns` of CPU time on `cpu`, ending at `now`, to the cell
    /// known by `key`, its domain and its capped nodes; returns the nodes
    /// whose cap it used up.
    fn charge(&mut self, cpu: usize, key: u32, ns: u64, now: u64) -> Vec<u32> {
        let Some(m) = self.cells.get_mut(&key) else {
            return Vec::new();
        };
        let clock = &mut self.clocks[cpu];
        if clock.weight != 0 {
            clock.vtime += Account::scaled(ns, clock.weight);
        }
        m.account.charge(ns);
        if let Some(a) = self.domains.get_mut(&m.domain) {
            a.charge(ns);
        }
        let period = now / PERIOD_NS;
        let mut spent = Vec::new();
        for node in &m.capped {
            let Some(cap) = self.caps.get_mut(node) else {
                continue;
            };
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
                spent.push(*node);
            }
        }
        spent
    }

    /// Whether the cell known by `key` counts against `node`'s cap.
    fn under(&self, key: u32, node: u32) -> bool {
        self.cells
            .get(&key)
            .is_some_and(|m| m.capped.contains(&node))
    }
}

impl Fair {
    /// Sets the CPU weight of `domain` (1 to 10,000): domains share each
    /// CPU in proportion to their weights.
    pub fn set_domain(&mut self, domain: u32, weight: u32) {
        let account = self
            .domains
            .entry(domain)
            .or_insert(Account::new(weight.into()));
        account.weight = u64::from(weight.max(1));
    }

    /// Forgets `domain`'s weight and virtual time (its last cell ended).
    pub fn forget_domain(&mut self, domain: u32) {
        self.domains.remove(&domain);
    }

    /// Shares the cell known by `key` by weight: it belongs to `domain`,
    /// runs `share` of its domain's time (out of [`WHOLE`], as [`flatten`]
    /// gives it) and counts against the caps of the `capped` nodes.
    pub fn set_cell(&mut self, key: u32, domain: u32, share: u64, capped: Vec<u32>) {
        let member = self.cells.entry(key).or_insert(Member {
            domain,
            account: Account::new(share),
            capped: Vec::new(),
        });
        if member.domain != domain {
            member.account.vtime = 0;
        }
        member.domain = domain;
        member.account.weight = share.max(1);
        member.capped = capped;
    }

    /// Forgets the cell known by `key` (it ended).
    pub fn forget_cell(&mut self, key: u32) {
        self.cells.remove(&key);
    }

    /// Forgets the cells and capped nodes whose identifier `keep` refuses
    /// (they left the tree), and the domains none of the others is in.
    pub fn retain(&mut self, keep: impl Fn(u32) -> bool) {
        self.cells.retain(|&key, _| keep(key));
        self.caps.retain(|&node, _| keep(node));
        let cells = &self.cells;
        self.domains
            .retain(|&d, _| cells.values().any(|m| m.domain == d));
    }

    /// Sets or removes the CPU cap of `node`: at most `cap_ns` of CPU time
    /// per [`PERIOD_NS`] for the cells that name it.
    pub fn set_cap(&mut self, node: u32, cap_ns: Option<u64>) {
        match cap_ns {
            Some(budget) => {
                let cap = self.caps.entry(node).or_insert(Cap {
                    budget,
                    period: 0,
                    used: 0,
                    until: None,
                });
                cap.budget = budget;
            }
            None => {
                self.caps.remove(&node);
            }
        }
    }

    /// The domain of the cell known by `key`, and the domain's and the
    /// cell's accounts (`None` when not shared by weight).
    fn accounts(&self, key: u32) -> (u32, Option<&Account>, Option<&Account>) {
        let Some(m) = self.cells.get(&key) else {
            return (u32::MAX, None, None);
        };
        (m.domain, self.domains.get(&m.domain), Some(&m.account))
    }
}

#[cfg(test)]
mod fair_shares {
    use super::*;
    use crate::{SLICE_NS, Scheduler};
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
        assert!(s.shares().cells.contains_key(&1));
        assert!(!s.shares().cells.contains_key(&0));
        assert!(s.shares().caps.is_empty(), "node 0 left");
        let domains: Vec<u32> = s.shares().domains.keys().copied().collect();
        assert_eq!(domains, [0], "domain 1 has no cell left");
    }

    proptest! {
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
