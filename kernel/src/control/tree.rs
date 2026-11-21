//! The control kernel's tree requests (protocol version 4, design D14 of
//! `add-cell-hierarchy`): placing the cells the host makes, moving them,
//! setting their policies and limits, subtree operations, and requests
//! scoped to a subtree (`rhizo scope`). The host names cells by its own
//! identifiers; the tree knows the kernel's.

use super::{Body, Entry, Error, Outcome, Place, Server, State};
use crate::audit::{self, Kind};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use rhizo_cap::{self as cap, Actor, HOST, Label};
use rhizo_control::{self as wire, CellInfo, Placement, Request, TreeInfo};

/// A tree refusal, for people.
pub(super) fn tree_error(e: cap::Error) -> (Error, String) {
    let (error, why) = match e {
        cap::Error::NotFound => (Error::NoCell, "no such cell in the tree"),
        cap::Error::Denied => (
            Error::Denied,
            "it widens what the parent allows, or the requester has no authority over the cell",
        ),
        cap::Error::Budget => (
            Error::Invalid,
            "the parent's free budget cannot hold these limits: give the cell memory, sockets \
             and descendants (--mem, --sockets, --cells) within it",
        ),
        cap::Error::Unlimited => (
            Error::Invalid,
            "the parent's memory, sockets or descendants are unlimited: limit them before it \
             has children",
        ),
        cap::Error::TooDeep => (Error::Invalid, "deeper than 16 cells below the host"),
        cap::Error::TooManyChildren => (Error::Invalid, "more than 1024 children"),
        cap::Error::Cycle => (
            Error::Invalid,
            "the new parent is in the cell's own subtree",
        ),
        cap::Error::Invalid => (
            Error::Invalid,
            "a value out of bounds (CPU weight 1 to 10000; an exclusive CPU set for a root only)",
        ),
        cap::Error::Exists => (Error::Failed, "the cell is in the tree already"),
        cap::Error::NoMemory => (Error::NoMemory, "the tree's records do not fit"),
    };
    (error, String::from(why))
}

fn invalid(why: String) -> (Error, String) {
    (Error::Invalid, why)
}

/// The tree's limits from the wire's.
fn limits_of(l: &wire::Limits) -> cap::Limits {
    cap::Limits {
        memory: l.memory,
        cpu_ns: l.cpu_ns,
        sockets: l.sockets,
        cells: l.cells,
    }
}

fn wire_limits(l: cap::Limits) -> wire::Limits {
    wire::Limits {
        memory: l.memory,
        cpu_ns: l.cpu_ns,
        sockets: l.sockets,
        cells: l.cells,
    }
}

/// The tree's egress policy from the wire's: each rule checked, with the
/// reason when one is refused (spec `control-plane`, malformed tree
/// request).
pub(super) fn egress_of(e: &wire::Egress) -> Result<cap::Egress, (Error, String)> {
    Ok(match e {
        wire::Egress::None => cap::Egress::None,
        wire::Egress::Full => cap::Egress::Full,
        wire::Egress::Rules(rules) => {
            if rules.len() > cap::limits::RULES_MAX {
                return Err(invalid(format!(
                    "{} egress rules; at most {}",
                    rules.len(),
                    cap::limits::RULES_MAX
                )));
            }
            let mut out = Vec::new();
            out.try_reserve_exact(rules.len())
                .map_err(|_| (Error::NoMemory, String::new()))?;
            for (k, r) in rules.iter().enumerate() {
                let proto = match r.proto {
                    wire::Proto::Tcp => cap::Proto::Tcp,
                    wire::Proto::Udp => cap::Proto::Udp,
                    wire::Proto::Any => cap::Proto::Any,
                };
                let rule =
                    cap::Rule::new(proto, r.addr, r.prefix, r.lo, r.hi).ok_or_else(|| {
                        invalid(if r.prefix > 32 {
                            format!("egress rule {}: prefix {} over 32", k + 1, r.prefix)
                        } else {
                            format!("egress rule {}: ports {} to {} are none", k + 1, r.lo, r.hi)
                        })
                    })?;
                out.push(rule);
            }
            cap::Egress::rules(&out).map_err(tree_error)?
        }
    })
}

/// The wire's egress policy from the tree's.
fn wire_egress(e: &cap::Egress) -> wire::Egress {
    match e {
        cap::Egress::None => wire::Egress::None,
        cap::Egress::Full => wire::Egress::Full,
        cap::Egress::Rules(rules) => wire::Egress::Rules(
            rules
                .iter()
                .map(|r| {
                    let ((addr, prefix), (lo, hi)) = (r.net(), r.ports());
                    wire::Rule {
                        proto: match r.proto() {
                            cap::Proto::Tcp => wire::Proto::Tcp,
                            cap::Proto::Udp => wire::Proto::Udp,
                            cap::Proto::Any => wire::Proto::Any,
                        },
                        addr,
                        prefix,
                        lo,
                        hi,
                    }
                })
                .collect(),
        ),
    }
}

/// What a placement asks of the tree, checked.
pub(super) fn spec_of(p: &Placement) -> Result<cap::Spec, (Error, String)> {
    let label = Label::new(&p.label).map_err(|_| {
        invalid(format!(
            "label `{}`: at most {} bytes of [A-Za-z0-9._-]",
            String::from_utf8_lossy(&p.label),
            cap::limits::LABEL_MAX
        ))
    })?;
    let weight = match p.cpu_weight {
        0 => cap::limits::WEIGHT_DEFAULT,
        w if (cap::limits::WEIGHT_MIN..=cap::limits::WEIGHT_MAX).contains(&w) => w,
        w => {
            return Err(invalid(format!(
                "CPU weight {w}: from {} to {}",
                cap::limits::WEIGHT_MIN,
                cap::limits::WEIGHT_MAX
            )));
        }
    };
    Ok(cap::Spec {
        limits: limits_of(&p.limits),
        on_parent_end: match p.on_parent_end {
            wire::OnParentEnd::Cascade => cap::OnParentEnd::Cascade,
            wire::OnParentEnd::Promote => cap::OnParentEnd::Promote,
        },
        egress: egress_of(&p.egress)?,
        ceiling: p.ceiling.as_ref().map(egress_of).transpose()?,
        label,
        weight,
        cpu_max: p.cpu_max,
        cpus: p.cpus,
    })
}

impl Server {
    /// The kernel cell of host cell `id` in the tree.
    pub(super) fn node_of(&self, id: u32) -> Result<u32, (Error, String)> {
        let e = self
            .cells
            .iter()
            .find(|e| e.id == id)
            .ok_or((Error::NoCell, String::new()))?;
        if matches!(e.place, Place::Ended(_)) {
            return Err((Error::NotRunning, String::from("the cell ended")));
        }
        if self.host.tree().get(e.cell).is_none() {
            return Err(invalid(format!(
                "cell {id} is not in the tree: a loaded cell joins it when it is resumed"
            )));
        }
        Ok(e.cell)
    }

    /// The host's identifier of kernel cell `cell` (0 if it has none).
    fn host_id(&self, cell: u32) -> u32 {
        self.cells
            .iter()
            .find(|e| e.cell == cell && !matches!(e.place, Place::Ended(_)))
            .map_or(0, |e| e.id)
    }

    /// Where a placement puts a cell: its parent in the tree and its spec,
    /// checked before the cell is made.
    pub(super) fn target(&self, p: &Placement) -> Result<(u32, cap::Spec), (Error, String)> {
        let parent = match p.parent {
            0 => HOST,
            id => self.node_of(id)?,
        };
        Ok((parent, spec_of(p)?))
    }

    /// The row of `e` in a list: its place in the tree, if it has one.
    pub(super) fn row(&self, e: &Entry) -> CellInfo {
        let node = match e.place {
            Place::Ended(_) => None,
            _ => self.host.tree().get(e.cell),
        };
        CellInfo {
            id: e.id,
            image: e.image.clone(),
            state: super::state_of(&e.place),
            tree: node.map(|n| TreeInfo {
                parent: if n.is_root() {
                    0
                } else {
                    self.host_id(n.parent())
                },
                root: self
                    .host
                    .tree()
                    .root_of(n.id())
                    .map_or(0, |r| self.host_id(r)),
                label: n.label().as_bytes().to_vec(),
                limits: wire_limits(n.limits()),
                egress: wire_egress(n.egress()),
            }),
        }
    }

    /// Ends cell `target` and its subtree.
    pub(super) fn kill_tree(&mut self, target: u32) -> Outcome {
        let cell = self.node_of(target)?;
        let sub = self.host.tree().subtree(cell).map_err(tree_error)?;
        self.host.kill_tree(cell, Actor::Host).map_err(tree_error)?;
        self.reap();
        // Dropped templates leave no ending behind.
        for e in &mut self.cells {
            if matches!(e.place, Place::Template(_)) && sub.contains(&e.cell) {
                e.place = Place::Ended(State::Killed);
            }
        }
        Ok(Body::None)
    }

    /// Suspends every running cell of `target`'s subtree.
    pub(super) fn suspend_tree(&mut self, target: u32) -> Outcome {
        let cell = self.node_of(target)?;
        let suspended = self
            .host
            .suspend_tree(cell, Actor::Host)
            .map_err(tree_error)?;
        self.reap();
        let mut ids = Vec::new();
        for (cell, snapshot) in suspended {
            match self
                .cells
                .iter_mut()
                .find(|e| e.cell == cell && matches!(e.place, Place::Live(_)))
            {
                Some(e) => {
                    e.place = Place::Suspended(snapshot);
                    ids.push(e.id);
                }
                // Every cell the host can name is in the table.
                None => {
                    crate::cell::close_parked(&snapshot);
                    self.host.drop_snapshot(&snapshot);
                }
            }
        }
        Ok(Body::Cells(ids))
    }

    /// Resumes every suspended cell of `target`'s subtree, parents first.
    pub(super) fn resume_tree(&mut self, target: u32) -> Outcome {
        let cell = self.node_of(target)?;
        let sub = self.host.tree().subtree(cell).map_err(tree_error)?;
        let suspended: Vec<u32> = sub
            .iter()
            .filter_map(|&c| {
                self.cells
                    .iter()
                    .find(|e| e.cell == c && matches!(e.place, Place::Suspended(_)))
                    .map(|e| e.id)
            })
            .collect();
        for &id in &suspended {
            self.resume(id)?;
        }
        Ok(Body::Cells(suspended))
    }

    pub(super) fn reparent(&mut self, target: u32, to: u32) -> Outcome {
        let cell = self.node_of(target)?;
        let to = match to {
            0 => HOST,
            id => self.node_of(id)?,
        };
        self.host
            .reparent(cell, to, Actor::Host)
            .map_err(tree_error)?;
        Ok(Body::None)
    }

    pub(super) fn policy(
        &mut self,
        target: u32,
        egress: &wire::Egress,
        ceiling: Option<&wire::Egress>,
    ) -> Outcome {
        let cell = self.node_of(target)?;
        let egress = egress_of(egress)?;
        let ceiling = match ceiling {
            Some(c) => egress_of(c)?,
            None => egress.clone(),
        };
        self.host
            .set_policy(cell, egress, ceiling, Actor::Host)
            .map_err(tree_error)?;
        Ok(Body::None)
    }

    pub(super) fn limits(&mut self, target: u32, limits: &wire::Limits) -> Outcome {
        let cell = self.node_of(target)?;
        self.host
            .set_tree_limits(cell, limits_of(limits), Actor::Host)
            .map_err(tree_error)?;
        Ok(Body::None)
    }

    /// Whether host cell `id` is `root` (a kernel cell) or below it.
    fn inside(&self, root: u32, id: u32) -> bool {
        self.node_of(id)
            .is_ok_and(|c| c == root || self.host.tree().is_ancestor(root, c))
    }

    /// `request` from a client limited to host cell `root`'s subtree
    /// (spec `control-plane`, scoped host access): refused unless every
    /// cell it names is in the subtree, it moves nothing out of it, and it
    /// widens no policy or limit. The kernel checks it, not only the
    /// host's server (design D14). The request is audited with its scope,
    /// let through or refused (design D18).
    pub(super) fn scoped(&mut self, id: u32, root: u32, request: Request) -> Outcome {
        let top = self.node_of(root)?;
        let (name, target) = super::about(&request);
        match self.within(top, root, &request) {
            Ok(None) => {
                audit::record(
                    Kind::Scoped,
                    top,
                    0,
                    format!("{name} cell {target}, scoped to cell {root}").as_bytes(),
                );
                self.execute(id, request)
            }
            Ok(Some(body)) => Ok(body),
            Err((error, why)) => {
                let kind = match error {
                    Error::Denied => Some(Kind::Scope),
                    Error::Invalid => Some(Kind::Tree),
                    _ => None,
                };
                if let Some(kind) = kind {
                    audit::deny(
                        kind,
                        top,
                        0,
                        format!("{name} cell {target}, scoped to cell {root}: {why}").as_bytes(),
                    );
                }
                Err((error, why))
            }
        }
    }

    /// Whether `request` stays inside the subtree of `top`, host cell
    /// `root`: `None` if it does and is to be executed, the reply of a
    /// list (only the subtree's rows).
    fn within(
        &self,
        top: u32,
        root: u32,
        request: &Request,
    ) -> Result<Option<Body>, (Error, String)> {
        let out = |what: &str| {
            Err((
                Error::Denied,
                format!("{what} is outside the subtree of cell {root}"),
            ))
        };
        let inside = |s: &Self, cell: u32| s.inside(top, cell);
        let placed = |s: &Self, p: &Placement| p.parent != 0 && inside(s, p.parent);
        match request {
            Request::List => {
                let rows = self
                    .cells
                    .iter()
                    .filter(|e| self.inside(top, e.id))
                    .map(|e| self.row(e))
                    .collect();
                return Ok(Some(Body::List(rows)));
            }
            Request::Run(run) if !placed(self, &run.placement) => {
                return out("the new cell's parent");
            }
            Request::Load { placement, .. } if !placed(self, placement) => {
                return out("the loaded cell's parent");
            }
            Request::Fork {
                cell, placement, ..
            } => {
                let template = matches!(
                    self.cells.iter().find(|e| e.id == *cell).map(|e| &e.place),
                    Some(Place::Template(_))
                );
                // A running cell's copies are its siblings: the root's
                // would be outside.
                let within = if template {
                    placed(self, placement)
                } else {
                    *cell != root
                };
                if !inside(self, *cell) || !within {
                    return out("the copies' place");
                }
            }
            Request::Kill(cell)
            | Request::KillTree(cell)
            | Request::Suspend(cell)
            | Request::SuspendTree(cell)
            | Request::Resume(cell)
            | Request::ResumeTree(cell)
            | Request::Output(cell)
            | Request::CloseInput(cell)
            | Request::Freeze(cell)
            | Request::WriteFile { cell, .. }
            | Request::Input { cell, .. }
            | Request::Save { cell, .. }
                if !inside(self, *cell) =>
            {
                return out(&format!("cell {cell}"));
            }
            Request::Reparent { cell, to } => {
                if *cell == root || !inside(self, *cell) || *to == 0 || !inside(self, *to) {
                    return out("the move");
                }
            }
            Request::Policy {
                cell,
                egress,
                ceiling,
            } => {
                if !inside(self, *cell) {
                    return out(&format!("cell {cell}"));
                }
                let n = self.host.tree().get(self.node_of(*cell)?);
                let new = egress_of(egress)?;
                let new_ceiling = match ceiling {
                    Some(c) => egress_of(c)?,
                    None => new.clone(),
                };
                if !n.is_some_and(|n| new.within(n.egress()) && new_ceiling.within(n.ceiling())) {
                    return Err((Error::Denied, String::from("a scoped client only narrows")));
                }
            }
            Request::Limits { cell, limits } => {
                if !inside(self, *cell) {
                    return out(&format!("cell {cell}"));
                }
                let now = self
                    .host
                    .tree()
                    .get(self.node_of(*cell)?)
                    .map(|n| n.limits());
                let l = limits_of(limits);
                if !now.is_some_and(|n| {
                    l.memory <= n.memory
                        && l.cpu_ns <= n.cpu_ns
                        && l.sockets <= n.sockets
                        && l.cells <= n.cells
                }) {
                    return Err((Error::Denied, String::from("a scoped client only narrows")));
                }
            }
            Request::Stats | Request::Shutdown | Request::Scoped { .. } => {
                return Err((
                    Error::Denied,
                    String::from("not for a client scoped to a subtree"),
                ));
            }
            _ => {}
        }
        Ok(None)
    }
}
