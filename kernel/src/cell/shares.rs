//! How the host's cells share the CPUs (design D23 of `add-cell-hierarchy`):
//! round robin while every cell is a root of the default weight without a
//! cap, where round robin gives each domain exactly its weight's part;
//! otherwise by weight, domains then the cells of each, within caps. The
//! scheduler's shares are computed from the tree when it changes, never
//! when a cell runs. So are the exclusive CPU sets (design D17): the CPUs
//! each domain with one runs on alone, the others running on the rest.

use super::*;
use rhizo_cap::Node;
use rhizo_cap::limits::WEIGHT_DEFAULT;
use rhizo_sched::fair::{self, PERIOD_NS};
use rhizo_sched::{Sets, Share as _};

/// Whether node `n` takes from round robin what its weight gives it: a
/// root (alone in its domain) of the default weight, without a cap.
fn even(n: &Node) -> bool {
    n.is_root() && n.weight() == WEIGHT_DEFAULT && n.cpu_max() == u32::MAX
}

/// Whether node `n` leaves the scheduler as round robin over every CPU:
/// [`even`], and without an exclusive CPU set.
fn plain(n: &Node) -> bool {
    even(n) && n.cpus() == 0
}

/// Whether a root placed with `spec` is plain (see [`plain`]).
pub(super) fn plain_root(spec: &rhizo_cap::Spec) -> bool {
    spec.weight == WEIGHT_DEFAULT && spec.cpu_max == u32::MAX && spec.cpus == 0
}

/// Whether root `id` may get the exclusive CPU set `set` (design D17): see
/// [`Sets::fits`], beside every other root's set.
pub(super) fn set_fits(tree: &rhizo_cap::Tree, id: u32, set: u64) -> bool {
    let others = tree.iter().filter(|n| n.is_root() && n.id() != id);
    let taken = others.fold(0, |t, n| t | n.cpus());
    Sets::fits(crate::smp::count(), taken, set)
}

/// A CPU cap in thousandths of a CPU, in nanoseconds of CPU time per
/// period (`u32::MAX`: none).
fn cap_ns(cpu_max: u32) -> Option<u64> {
    (cpu_max != u32::MAX).then(|| u64::from(cpu_max) * PERIOD_NS / 1000)
}

/// What the scheduler keeps of one node: its identifier, domain, weight if
/// it is its domain's root, flattened share, capped nodes and cap.
type Shares = (u32, u32, Option<u32>, u64, Vec<u32>, Option<u64>);

impl Host {
    /// Cells entered or left the tree: while cells share by weight, the
    /// shares again. Round robin needs none, and keeps holding: a root of
    /// the default weight, a copy of a plain cell and a cell resumed where
    /// it was are plain again.
    /// A cell was placed, `plain` if as a root with [`plain_root`]'s spec:
    /// the shares again if cells share by weight or it is not plain (on
    /// round robin, a plain root tests two values it already holds).
    #[inline(always)]
    pub(super) fn reshare_after(&mut self, plain: bool) {
        if self.by_weight || !plain {
            self.reshare();
        }
    }

    #[inline(always)]
    pub(super) fn reshare_if_by_weight(&mut self) {
        if self.by_weight {
            self.reshare();
        }
    }

    /// Computes from the tree how cells share the CPUs: round robin over
    /// every CPU while every node is plain; otherwise the exclusive sets,
    /// and, unless every node is [`even`], each cell's domain, flattened
    /// share and capped ancestors, each domain's weight and each cap. Time
    /// used so far stays counted.
    #[cold]
    #[inline(never)]
    pub(super) fn reshare(&mut self) {
        self.by_weight = !self.tree.iter().all(plain);
        let sets = self.sets();
        self.locked(|s| {
            if sets.is_some() || s.sched.sets().is_some() {
                s.sched.set_sets(sets);
            }
        });
        if self.tree.iter().all(even) {
            self.locked(|s| *s.sched.shares() = None);
            return;
        }
        let nodes: Vec<&Node> = self.tree.iter().collect();
        let at = |id: u32| nodes.binary_search_by_key(&id, |n| n.id()).ok();
        let flat: Vec<fair::Node> = nodes
            .iter()
            .map(|n| fair::Node {
                parent: if n.is_root() { None } else { at(n.parent()) },
                weight: n.weight().into(),
                cap_ns: cap_ns(n.cpu_max()),
            })
            .collect();
        let shares: Vec<Shares> = fair::flatten(&flat)
            .into_iter()
            .zip(&nodes)
            .map(|(place, n)| {
                let capped = place.capped.iter().map(|&i| nodes[i].id()).collect();
                let root = n.is_root().then(|| n.weight().into());
                (
                    n.id(),
                    n.domain(),
                    root,
                    place.share,
                    capped,
                    cap_ns(n.cpu_max()),
                )
            })
            .collect();
        let cpus = crate::smp::count();
        let ids: Vec<u32> = shares.iter().map(|s| s.0).collect(); // sorted
        self.locked(|s| {
            let f = s.sched.shares().get_or_insert_with(|| Fair::new(cpus));
            f.retain(|id| ids.binary_search(&id).is_ok());
            for (id, domain, root, share, capped, cap) in shares {
                if let Some(weight) = root {
                    f.set_domain(domain, weight);
                }
                f.set_cell(id, domain, share, capped);
                f.set_cap(id, cap);
            }
        });
    }

    /// The exclusive CPU sets of the tree's cells (design D17): each cell
    /// of a domain whose root has one, with it; `None` when no root has
    /// one. The host gives only sets that fit ([`set_fits`]).
    fn sets(&self) -> Option<Sets> {
        let roots = self.tree.iter().filter(|n| n.is_root() && n.cpus() != 0);
        let mut sets: Vec<(u32, u64)> = roots.map(|n| (n.domain(), n.cpus())).collect();
        if sets.is_empty() {
            return None;
        }
        sets.sort_unstable();
        let set = |n: &Node| {
            let at = sets.binary_search_by_key(&n.domain(), |s| s.0).ok()?;
            Some((n.id(), sets[at].1))
        };
        Sets::new(
            crate::smp::count(),
            self.tree.iter().filter_map(set).collect(),
        )
    }

    /// The CPU time live cell `id` has used, counted while it has a CPU
    /// time quota (in-kernel tests and benchmarks).
    #[cfg(any(feature = "ktest", feature = "bench-cell"))]
    pub fn cpu_ns(&mut self, id: u32) -> Option<u64> {
        self.with_live(id, |live| live.cell.cpu_ns)
    }
}
