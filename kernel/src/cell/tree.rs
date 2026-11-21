//! The ownership tree of the host's cells (spec `cell-hierarchy`, design D1
//! to D3 of `add-cell-hierarchy`): where each cell goes when it is made,
//! its domain and its number there, and what its end does to its subtree.

use super::*;
use rhizo_cap::{Limits, Spec, Tree};

/// A fresh domain identifier: from the cells' own counter, so never 0 and
/// never one another domain had in this boot.
fn fresh_domain() -> u32 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// The limits of a root with `quota`.
fn root_limits(quota: &Quota) -> Limits {
    Limits {
        memory: quota.memory,
        cpu_ns: quota.cpu_ns,
        sockets: quota.sockets,
        cells: u32::MAX,
    }
}

impl Host {
    /// The tree of this host's cells.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
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
        let spec = Spec {
            limits: root_limits(&cell.quota),
            ..Spec::default()
        };
        let Ok(pid) = self.tree.add_root(cell.id, spec, 0) else {
            return false;
        };
        cell.renumber(pid);
        cell.domain = cell.id;
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
        let parent = self.tree.get(source).map_or(0, |n| n.parent());
        let used = self.usage(parent);
        let mut placed = |k: usize, pid, domain| {
            if let Some(copy) = copies.get_mut(k) {
                (copy.cell.pid, copy.cell.domain) = (pid, domain);
            }
        };
        self.tree
            .add_copies(source, ids, &|_| used, &mut placed)
            .is_ok()
    }

    /// Places a cell restored from `snapshot`'s state, not yet run: under
    /// the parent it had, with its domain and number, if it was suspended
    /// from this host; otherwise as the root of a new domain (spec
    /// `snapshots`, loaded snapshots hold no authority). Its threads keep
    /// their identifiers. `false` if the tree has no room.
    pub(super) fn enter_restored(&mut self, cell: &mut Cell, origin: u32) -> bool {
        if let Some(at) = self.suspended.iter().position(|&s| s == origin)
            && let Some(n) = self.tree.rekey(origin, cell.id)
        {
            (cell.pid, cell.domain) = (n.pid(), n.domain());
            self.suspended.swap_remove(at);
            return true;
        }
        let spec = Spec {
            limits: root_limits(&cell.quota),
            ..Spec::default()
        };
        let Ok(pid) = self.tree.add_root(cell.id, spec, 0) else {
            return false;
        };
        (cell.pid, cell.domain) = (pid, cell.id);
        true
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
            self.suspended.swap_remove(at);
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
            let Ok(ended) = self.tree.end(id, &mut fresh_domain) else {
                continue;
            };
            for &gone in &ended.gone {
                if let Some(at) = self.suspended.iter().position(|&s| s == gone) {
                    self.suspended.swap_remove(at);
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
                    let Some(n) = self.tree.get(cell) else {
                        continue;
                    };
                    let (pid, domain) = (n.pid(), n.domain());
                    self.with_live(cell, |live| {
                        (live.cell.pid, live.cell.domain) = (pid, domain)
                    });
                    if let Some((c, ..)) = self.templates.iter_mut().find(|(c, ..)| c.id == cell) {
                        (c.pid, c.domain) = (pid, domain);
                    }
                }
            }
        }
        self.settling = false;
    }
}
