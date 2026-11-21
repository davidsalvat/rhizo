//! How the host's cells share the CPUs (design D23 of `add-cell-hierarchy`):
//! round robin while every cell is a root of the default weight without a
//! cap, where round robin gives each domain exactly its weight's part;
//! otherwise by weight, domains then the cells of each, within caps. The
//! scheduler's shares are computed from the tree when it changes, never
//! when a cell runs.

use super::*;
use rhizo_cap::Node;
use rhizo_cap::limits::WEIGHT_DEFAULT;
use rhizo_sched::Share as _;
use rhizo_sched::fair::{self, PERIOD_NS};

/// Whether node `n` takes from round robin what its weight gives it: a
/// root (alone in its domain) of the default weight, without a cap.
fn plain(n: &Node) -> bool {
    n.is_root() && n.weight() == WEIGHT_DEFAULT && n.cpu_max() == u32::MAX
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
    #[inline(always)]
    pub(super) fn reshare_if_by_weight(&mut self) {
        if self.by_weight {
            self.reshare();
        }
    }

    /// Computes from the tree how cells share the CPUs: round robin while
    /// every node is plain; otherwise each cell's domain, flattened share
    /// and capped ancestors, each domain's weight and each cap. Time used so
    /// far stays counted.
    #[cold]
    #[inline(never)]
    pub(super) fn reshare(&mut self) {
        self.by_weight = !self.tree.iter().all(plain);
        if !self.by_weight {
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

    /// The CPU time live cell `id` has used, counted while it has a CPU
    /// time quota (in-kernel tests and benchmarks).
    #[cfg(any(feature = "ktest", feature = "bench-cell"))]
    pub fn cpu_ns(&mut self, id: u32) -> Option<u64> {
        self.with_live(id, |live| live.cell.cpu_ns)
    }
}
