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
    #[inline]
    pub(super) fn reshare_if_by_weight(&mut self) {
        if self.locked(|s| s.sched.shares().is_some()) {
            self.reshare();
        }
    }

    /// Computes from the tree how cells share the CPUs: round robin while
    /// every node is plain; otherwise each cell's domain, flattened share
    /// and capped ancestors, each domain's weight and each cap. Time used so
    /// far stays counted.
    #[inline(never)]
    pub(super) fn reshare(&mut self) {
        if self.tree.iter().all(plain) {
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

    /// Makes live cell `id`, a root without children, a root again or a
    /// child of `parent` (`None`: none), with CPU weight `weight` and CPU
    /// cap `cpu_max` (thousandths of a CPU, `u32::MAX`: none), and shares
    /// the CPUs again; `false` if the tree refuses. A parent without bounded
    /// limits gets the largest ones a test needs. What `rhizo run
    /// --cpu-weight`, `--cpu-max` and a placement under a parent will set
    /// once the control protocol carries them (version 4); in-kernel tests
    /// and benchmarks only until then.
    #[cfg(any(feature = "ktest", feature = "bench-cell"))]
    pub fn set_cpu_share(
        &mut self,
        id: u32,
        parent: Option<u32>,
        weight: u16,
        cpu_max: u32,
    ) -> bool {
        use rhizo_cap::{Actor, Limits, Spec, limits::CHILDREN_MAX};
        let Some(n) = self
            .tree
            .get(id)
            .filter(|n| n.is_root() && n.children() == 0)
        else {
            return false;
        };
        let saved = Spec {
            limits: n.limits(),
            weight: n.weight(),
            cpu_max: n.cpu_max(),
            ..Spec::default()
        };
        let mut spec = Spec {
            weight,
            cpu_max,
            ..saved.clone()
        };
        let bounded = Limits {
            memory: 1 << 40,
            cpu_ns: u64::MAX,
            sockets: 1 << 16,
            cells: CHILDREN_MAX.into(),
        };
        if let Some(p) = parent {
            let Some(pn) = self.tree.get(p) else {
                return false;
            };
            if pn.limits().memory == u64::MAX {
                let free = |_| Limits::ZERO;
                if self
                    .tree
                    .set_limits(p, bounded, Actor::Host, &free)
                    .is_err()
                {
                    return false;
                }
            }
            spec.limits = Limits {
                memory: 1 << 32,
                sockets: 64,
                cells: 0,
                ..bounded
            };
        }
        let _ = self.tree.end(id, &mut || 0); // a leaf: nothing fresh
        let placed = match parent {
            None => self.tree.add_root(id, spec, 0).is_ok(),
            Some(p) => {
                let free = |_| Limits::ZERO;
                self.tree.add_child(id, p, spec, Actor::Host, &free).is_ok()
            }
        };
        if !placed {
            let _ = self.tree.add_root(id, saved, 0);
        }
        let (pid, domain) = self.tree.get(id).map_or((0, 0), |n| (n.pid(), n.domain()));
        self.with_live(id, |live| (live.cell.pid, live.cell.domain) = (pid, domain));
        self.reshare();
        placed
    }

    /// The CPU time live cell `id` has used, counted while it has a CPU
    /// time quota (in-kernel tests and benchmarks).
    #[cfg(any(feature = "ktest", feature = "bench-cell"))]
    pub fn cpu_ns(&mut self, id: u32) -> Option<u64> {
        self.with_live(id, |live| live.cell.cpu_ns)
    }
}
