//! The ownership tree (design D1 to D4 of `add-cell-hierarchy`).

use crate::limits::{CHILDREN_MAX, DEPTH_MAX, LABEL_MAX, WEIGHT_DEFAULT, WEIGHT_MAX, WEIGHT_MIN};
use crate::{Egress, Error, reserve};
use alloc::boxed::Box;
use alloc::vec::Vec;

/// The parent of a root: the host, which is not a cell.
pub const HOST: u32 = 0;

/// Who asks for a tree operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    /// The host: authority over every cell.
    Host,
    /// A cell: authority over its descendants only (design D2).
    Cell(u32),
}

/// What a cell may use: memory in bytes, CPU time in nanoseconds, sockets,
/// and descendants. `u64::MAX` or `u32::MAX` is no limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub memory: u64,
    pub cpu_ns: u64,
    pub sockets: u32,
    pub cells: u32,
}

impl Limits {
    pub const UNLIMITED: Self = Self {
        memory: u64::MAX,
        cpu_ns: u64::MAX,
        sockets: u32::MAX,
        cells: u32::MAX,
    };
    pub const ZERO: Self = Self {
        memory: 0,
        cpu_ns: 0,
        sockets: 0,
        cells: 0,
    };

    /// Whether a cell with these limits may have children: memory, sockets
    /// and descendants all bounded (design D4, no fork bombs).
    fn bounded(&self) -> bool {
        self.memory != u64::MAX && self.sockets != u32::MAX && self.cells != u32::MAX
    }

    /// What a child with these limits takes from its parent: its limits,
    /// and one descendant more than its own descendant limit.
    fn charge(&self) -> Self {
        Self {
            cells: self.cells.saturating_add(1),
            ..*self
        }
    }

    fn zip(a: Self, b: Self, f64: impl Fn(u64, u64) -> u64, f32: impl Fn(u32, u32) -> u32) -> Self {
        Self {
            memory: f64(a.memory, b.memory),
            cpu_ns: f64(a.cpu_ns, b.cpu_ns),
            sockets: f32(a.sockets, b.sockets),
            cells: f32(a.cells, b.cells),
        }
    }

    fn add(self, b: Self) -> Self {
        Self::zip(self, b, u64::saturating_add, u32::saturating_add)
    }

    /// `self - b` where `self` is a limit: no limit stays no limit.
    fn left(self, b: Self) -> Self {
        Self::zip(
            self,
            b,
            |l, x| {
                if l == u64::MAX {
                    l
                } else {
                    l.saturating_sub(x)
                }
            },
            |l, x| {
                if l == u32::MAX {
                    l
                } else {
                    l.saturating_sub(x)
                }
            },
        )
    }

    /// Whether `self` (what is asked) fits in `room` on every bounded field.
    fn fits(self, room: Self) -> bool {
        (room.memory == u64::MAX || self.memory <= room.memory)
            && (room.cpu_ns == u64::MAX || self.cpu_ns <= room.cpu_ns)
            && (room.sockets == u32::MAX || self.sockets <= room.sockets)
            && (room.cells == u32::MAX || self.cells <= room.cells)
    }
}

/// How a child's limits come from its parent's: only `Reserved`, in full,
/// at creation (the user's decision 1). The field leaves room for other
/// modes, which a later change would add with its own analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Reserved,
}

/// What happens to a cell when its parent ends (design D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnParentEnd {
    /// It ends too, with its descendants.
    #[default]
    Cascade,
    /// It becomes a root with its subtree.
    Promote,
}

/// A cell's label: at most [`LABEL_MAX`] bytes of `[A-Za-z0-9._-]`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Label(Box<[u8]>);

impl Label {
    /// The label `bytes`, or [`Error::Invalid`].
    pub fn new(bytes: &[u8]) -> Result<Self, Error> {
        let ok = |b: &u8| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-');
        if bytes.len() > LABEL_MAX || !bytes.iter().all(ok) {
            return Err(Error::Invalid);
        }
        let mut v = Vec::new();
        reserve(&mut v, bytes.len())?;
        v.extend_from_slice(bytes);
        Ok(Self(v.into_boxed_slice()))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// What the creator of a cell decides for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub limits: Limits,
    pub on_parent_end: OnParentEnd,
    pub egress: Egress,
    /// Its exposure ceiling (design D8); `None` takes the egress policy.
    pub ceiling: Option<Egress>,
    pub label: Label,
    /// CPU weight, from [`WEIGHT_MIN`] to [`WEIGHT_MAX`] (design D23).
    pub weight: u16,
    /// CPU cap for the whole subtree in thousandths of a CPU (`u32::MAX`:
    /// none), at most the parent's (design D23).
    pub cpu_max: u32,
    /// An exclusive CPU set, one bit per CPU, roots only (design D17; 0:
    /// none).
    pub cpus: u64,
}

impl Default for Spec {
    fn default() -> Self {
        Self {
            limits: Limits::UNLIMITED,
            on_parent_end: OnParentEnd::Cascade,
            egress: Egress::None,
            ceiling: None,
            label: Label::default(),
            weight: WEIGHT_DEFAULT,
            cpu_max: u32::MAX,
            cpus: 0,
        }
    }
}

/// One cell in the tree: a running or suspended cell, or a template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    id: u32,
    parent: u32,
    first_child: u32,
    next_sibling: u32,
    children: u16,
    depth: u8,
    on_parent_end: OnParentEnd,
    domain: u32,
    pid: u32,
    /// For a root: the number the next cell made in its domain gets.
    next_pid: u32,
    limits: Limits,
    /// What its children took: the sum of their charges.
    reserved: Limits,
    weight: u16,
    cpu_max: u32,
    cpus: u64,
    egress: Egress,
    ceiling: Egress,
    label: Label,
}

impl Node {
    /// The cell's identifier in the kernel.
    pub fn id(&self) -> u32 {
        self.id
    }
    /// Its parent, [`HOST`] for a root.
    pub fn parent(&self) -> u32 {
        self.parent
    }
    pub fn is_root(&self) -> bool {
        self.parent == HOST
    }
    /// Its depth below the host: 1 for a root.
    pub fn depth(&self) -> u8 {
        self.depth
    }
    /// Its domain's identifier: never 0, unique among the domains that
    /// ever lived while the kernel's fresh identifiers are (see
    /// [`Tree::add_root`]), and unchanged while the domain lives.
    pub fn domain(&self) -> u32 {
        self.domain
    }
    /// Its number inside its domain, what `getpid` returns: 1 for a root
    /// made as one, then in the order cells join the domain.
    pub fn pid(&self) -> u32 {
        self.pid
    }
    pub fn limits(&self) -> Limits {
        self.limits
    }
    /// What its children reserved.
    pub fn reserved(&self) -> Limits {
        self.reserved
    }
    /// What the cell itself may use: its limits less its children's
    /// reservations.
    pub fn own(&self) -> Limits {
        self.limits.left(self.reserved)
    }
    /// Its free budget while it uses `used` itself (design D4): its limits
    /// less what it uses and what its children reserved.
    pub fn free(&self, used: Limits) -> Limits {
        self.own().left(used)
    }
    pub fn mode(&self) -> Mode {
        Mode::Reserved
    }
    pub fn on_parent_end(&self) -> OnParentEnd {
        self.on_parent_end
    }
    pub fn egress(&self) -> &Egress {
        &self.egress
    }
    pub fn ceiling(&self) -> &Egress {
        &self.ceiling
    }
    pub fn label(&self) -> &Label {
        &self.label
    }
    pub fn weight(&self) -> u16 {
        self.weight
    }
    pub fn cpu_max(&self) -> u32 {
        self.cpu_max
    }
    pub fn cpus(&self) -> u64 {
        self.cpus
    }
    /// How many children it has.
    pub fn children(&self) -> u16 {
        self.children
    }
}

/// What [`Tree::end`] did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ended {
    /// The cells that ended: the one asked for, and every descendant whose
    /// policy was to cascade, depth first (children before parents).
    pub gone: Vec<u32>,
    /// The cells that became roots, with their subtrees (each in a new
    /// domain, numbered again).
    pub promoted: Vec<u32>,
}

/// The ownership tree of a kernel's cells (design D1), ordered by
/// identifier.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tree {
    nodes: Vec<Node>,
}

/// Fresh domain identifiers: never 0, never one a live domain has.
pub type Fresh<'a> = &'a mut dyn FnMut() -> u32;
/// What a cell uses itself, as limits (descendants: 0).
pub type Usage<'a> = &'a dyn Fn(u32) -> Limits;

impl Tree {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of cells in the tree.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    fn index(&self, id: u32) -> Option<usize> {
        self.nodes.binary_search_by_key(&id, |n| n.id).ok()
    }

    /// Cell `id`'s node.
    pub fn get(&self, id: u32) -> Option<&Node> {
        self.index(id).map(|i| &self.nodes[i])
    }

    fn node(&self, id: u32) -> &Node {
        &self.nodes[self.index(id).expect("a cell of the tree")]
    }

    fn node_mut(&mut self, id: u32) -> &mut Node {
        let i = self.index(id).expect("a cell of the tree");
        &mut self.nodes[i]
    }

    /// The domain of cell `id`.
    pub fn domain(&self, id: u32) -> Option<u32> {
        self.get(id).map(|n| n.domain)
    }

    /// The root of cell `id`'s domain.
    pub fn root_of(&self, mut id: u32) -> Option<u32> {
        loop {
            let parent = self.get(id)?.parent;
            if parent == HOST {
                return Some(id);
            }
            id = parent;
        }
    }

    /// Every node, by identifier.
    pub fn iter(&self) -> impl Iterator<Item = &Node> {
        self.nodes.iter()
    }

    /// The children of cell `id`, newest first ([`HOST`]: the roots).
    pub fn children(&self, id: u32) -> impl Iterator<Item = u32> + '_ {
        let host = (id == HOST).then(|| self.nodes.iter().filter(|n| n.is_root()).map(|n| n.id));
        let mut next = if id == HOST {
            HOST
        } else {
            self.get(id).map_or(HOST, |n| n.first_child)
        };
        let cells = core::iter::from_fn(move || {
            (next != HOST).then(|| {
                let id = next;
                next = self.node(id).next_sibling;
                id
            })
        });
        host.into_iter().flatten().chain(cells)
    }

    /// Whether `a` is a strict ancestor of `b`.
    pub fn is_ancestor(&self, a: u32, b: u32) -> bool {
        let mut at = b;
        while let Some(n) = self.get(at) {
            if n.parent == a && a != HOST {
                return true;
            }
            at = n.parent;
        }
        false
    }

    /// Whether `by` may act on cell `target`: the host always, a cell only
    /// on its descendants, never on itself or an ancestor (design D2).
    pub fn controls(&self, by: Actor, target: u32) -> bool {
        self.get(target).is_some()
            && match by {
                Actor::Host => true,
                Actor::Cell(a) => self.is_ancestor(a, target),
            }
    }

    /// The subtree of cell `id`, itself first, parents before children.
    pub fn subtree(&self, id: u32) -> Result<Vec<u32>, Error> {
        if self.get(id).is_none() {
            return Err(Error::NotFound);
        }
        let mut out = Vec::new();
        reserve(&mut out, 1)?;
        out.push(id);
        let mut i = 0;
        while i < out.len() {
            let at = out[i];
            for c in self.children(at) {
                if out.len() == out.capacity() {
                    let more = out.len();
                    reserve(&mut out, more)?;
                }
                out.push(c);
            }
            i += 1;
        }
        Ok(out)
    }

    /// How many levels the subtree of `id` has below it.
    fn height(&self, sub: &[u32]) -> u8 {
        let top = self.node(sub[0]).depth;
        sub.iter()
            .map(|&c| self.node(c).depth - top)
            .max()
            .unwrap_or(0)
    }

    /// The free budget of cell `id`, which uses `used` itself.
    fn free(&self, id: u32, used: Limits) -> Limits {
        self.node(id).free(used)
    }

    /// Adds root `id` of a new domain `domain` (a fresh identifier, never
    /// 0). Its numbering starts at 1, skipping `skip` (the source's number
    /// for a copy, so a copy's `getpid` differs, design D22; 0 for none).
    pub fn add_root(&mut self, id: u32, domain: u32, spec: Spec, skip: u32) -> Result<(), Error> {
        if id == HOST || domain == 0 {
            return Err(Error::Invalid);
        }
        let node = self.made(id, HOST, domain, spec)?;
        let i = self.insert_at(id)?;
        self.nodes.insert(i, node);
        let pid = self.number(id, skip);
        self.node_mut(id).pid = pid;
        Ok(())
    }

    /// Adds cell `id` as a child of `parent`, at `by`'s request: its limits
    /// are carved from the parent's free budget (the parent uses
    /// `usage(parent)` itself), its egress and ceiling must be within the
    /// parent's and its CPU cap at most the parent's.
    pub fn add_child(
        &mut self,
        id: u32,
        parent: u32,
        spec: Spec,
        by: Actor,
        usage: Usage,
    ) -> Result<(), Error> {
        let p = self.get(parent).ok_or(Error::NotFound)?;
        if by != Actor::Host && by != Actor::Cell(parent) && !self.controls(by, parent) {
            return Err(Error::Denied);
        }
        if spec.cpus != 0 {
            return Err(Error::Invalid); // exclusive sets go to roots
        }
        let ceiling = spec.ceiling.as_ref().unwrap_or(&spec.egress);
        if !spec.egress.within(&p.egress) || !ceiling.within(&p.ceiling) || spec.cpu_max > p.cpu_max
        {
            return Err(Error::Denied);
        }
        self.room_for(parent, spec.limits.charge(), 1, usage)?;
        if p.depth >= DEPTH_MAX {
            return Err(Error::TooDeep);
        }
        let node = self.made(id, parent, p.domain, spec)?;
        let i = self.insert_at(id)?;
        self.nodes.insert(i, node);
        self.link(id, parent);
        let pid = self.number(id, 0);
        self.node_mut(id).pid = pid;
        Ok(())
    }

    /// Whether `parent` can take `count` more children charging `charge`
    /// each.
    fn room_for(&self, parent: u32, charge: Limits, count: u32, usage: Usage) -> Result<(), Error> {
        let p = self.node(parent);
        if !p.limits.bounded() {
            return Err(Error::Unlimited);
        }
        if u32::from(p.children) + count > u32::from(CHILDREN_MAX) {
            return Err(Error::TooManyChildren);
        }
        let mut all = Limits::ZERO;
        for _ in 0..count {
            all = all.add(charge);
        }
        if !all.fits(self.free(parent, usage(parent))) {
            return Err(Error::Budget);
        }
        Ok(())
    }

    /// A new node for `spec`, checked, not yet in the tree.
    fn made(&self, id: u32, parent: u32, domain: u32, spec: Spec) -> Result<Node, Error> {
        if self.get(id).is_some() {
            return Err(Error::Exists);
        }
        if !(WEIGHT_MIN..=WEIGHT_MAX).contains(&spec.weight) {
            return Err(Error::Invalid);
        }
        let ceiling = spec.ceiling.unwrap_or_else(|| spec.egress.clone());
        let depth = if parent == HOST {
            1
        } else {
            self.node(parent).depth + 1
        };
        Ok(Node {
            id,
            parent,
            first_child: HOST,
            next_sibling: HOST,
            children: 0,
            depth,
            on_parent_end: spec.on_parent_end,
            domain,
            pid: 0,
            next_pid: 1,
            limits: spec.limits,
            reserved: Limits::ZERO,
            weight: spec.weight,
            cpu_max: spec.cpu_max,
            cpus: spec.cpus,
            egress: spec.egress,
            ceiling,
            label: spec.label,
        })
    }

    /// Where `id` goes in the nodes, with room for it.
    fn insert_at(&mut self, id: u32) -> Result<usize, Error> {
        let at = self
            .nodes
            .binary_search_by_key(&id, |n| n.id)
            .err()
            .ok_or(Error::Exists)?;
        reserve(&mut self.nodes, 1)?;
        Ok(at)
    }

    /// Links node `id` under `parent` (a cell) and counts its reservation.
    fn link(&mut self, id: u32, parent: u32) {
        let p = self.node_mut(parent);
        let first = core::mem::replace(&mut p.first_child, id);
        p.children += 1;
        let n = self.node_mut(id);
        n.parent = parent;
        n.next_sibling = first;
        self.recount(parent);
    }

    /// Unlinks node `id` from its parent's children (a root: nothing).
    fn unlink(&mut self, id: u32) {
        let (parent, next) = {
            let n = self.node(id);
            (n.parent, n.next_sibling)
        };
        if parent == HOST {
            return;
        }
        if self.node(parent).first_child == id {
            self.node_mut(parent).first_child = next;
        } else {
            let mut at = self.node(parent).first_child;
            while self.node(at).next_sibling != id {
                at = self.node(at).next_sibling;
            }
            self.node_mut(at).next_sibling = next;
        }
        self.node_mut(parent).children -= 1;
        let n = self.node_mut(id);
        n.parent = HOST;
        n.next_sibling = HOST;
        self.recount(parent);
    }

    /// Sets cell `id`'s reservation to the sum of its children's charges.
    fn recount(&mut self, id: u32) {
        let reserved = self
            .children(id)
            .fold(Limits::ZERO, |sum, c| sum.add(self.node(c).limits.charge()));
        self.node_mut(id).reserved = reserved;
    }

    /// The next number in cell `id`'s domain, never `skip`.
    fn number(&mut self, id: u32, skip: u32) -> u32 {
        let root = self.root_of(id).expect("a cell of the tree");
        let r = self.node_mut(root);
        let mut pid = r.next_pid;
        if pid == skip {
            pid += 1;
        }
        r.next_pid = pid + 1;
        pid
    }

    /// Adds the cells `ids` as copies of `source` (design D13): siblings of
    /// it, all or none. Copies of a root are roots, each of a new domain
    /// from `fresh`, numbered from 1 but never with the source's number;
    /// copies of any other cell are carved from its parent's budget again.
    /// A copy has the source's limits and policies, and no children.
    pub fn add_copies(
        &mut self,
        source: u32,
        ids: &[u32],
        usage: Usage,
        fresh: Fresh,
    ) -> Result<(), Error> {
        let s = self.get(source).ok_or(Error::NotFound)?;
        let spec = Spec {
            limits: s.limits,
            on_parent_end: s.on_parent_end,
            egress: s.egress.clone(),
            ceiling: Some(s.ceiling.clone()),
            label: s.label.clone(),
            weight: s.weight,
            cpu_max: s.cpu_max,
            cpus: 0, // an exclusive set stays with one domain
        };
        let (parent, domain, pid) = (s.parent, s.domain, s.pid);
        let count = u32::try_from(ids.len()).map_err(|_| Error::Invalid)?;
        if parent != HOST {
            self.room_for(parent, spec.limits.charge(), count, usage)?;
        }
        // Every check and allocation first: then nothing fails.
        let mut nodes = Vec::new();
        reserve(&mut nodes, ids.len())?;
        for (k, &id) in ids.iter().enumerate() {
            if ids[..k].contains(&id) {
                return Err(Error::Exists);
            }
            nodes.push(self.made(id, parent, domain, spec.clone())?);
        }
        reserve(&mut self.nodes, ids.len())?;
        let domains: Vec<u32> = if parent == HOST {
            let mut d = Vec::new();
            reserve(&mut d, ids.len())?;
            d.extend(ids.iter().map(|_| fresh()));
            d
        } else {
            Vec::new()
        };
        for (k, mut node) in nodes.into_iter().enumerate() {
            let id = node.id;
            if parent == HOST {
                node.domain = domains[k];
            }
            let i = self.nodes.binary_search_by_key(&id, |n| n.id).unwrap_err();
            self.nodes.insert(i, node);
            if parent != HOST {
                self.link(id, parent);
            }
            let p = self.number(id, if parent == HOST { pid } else { 0 });
            self.node_mut(id).pid = p;
        }
        Ok(())
    }

    /// Gives cell `old` the identifier `new` (a resume gives the cell a new
    /// kernel identifier); its place, domain and number stay. Returns
    /// whether `old` was in the tree and `new` was not.
    pub fn rekey(&mut self, old: u32, new: u32) -> bool {
        if new == HOST || self.get(new).is_some() {
            return false;
        }
        let Some(i) = self.index(old) else {
            return false;
        };
        let mut node = self.nodes.remove(i);
        node.id = new;
        let (parent, first) = (node.parent, node.first_child);
        let at = self.nodes.binary_search_by_key(&new, |n| n.id).unwrap_err();
        self.nodes.insert(at, node); // the room `old` left
        if parent != HOST {
            let p = self.node_mut(parent);
            if p.first_child == old {
                p.first_child = new;
            } else {
                let mut at = p.first_child;
                while self.node(at).next_sibling != old {
                    at = self.node(at).next_sibling;
                }
                self.node_mut(at).next_sibling = new;
            }
        }
        let mut c = first;
        while c != HOST {
            let n = self.node_mut(c);
            n.parent = new;
            c = n.next_sibling;
        }
        true
    }

    /// Moves cell `id` under `to` ([`HOST`]: makes it a root), at `by`'s
    /// request (design D2). Refused unless `by` controls the cell (and, for
    /// a cell parent, is that parent or controls it), the new parent is
    /// outside the cell's subtree and, unless `by` is the host, in the same
    /// domain; the depth, the parent's children and its free budget must
    /// hold the subtree. The subtree's egress and ceiling become their
    /// intersection with the new parent's. A cell made a root at a cell's
    /// request takes its reservation out of every ancestor's limits for
    /// good, so promotion creates nothing; at the host's request the old
    /// parent gets it back. A subtree that changes domain is numbered
    /// again in it, its top cell first.
    pub fn reparent(
        &mut self,
        id: u32,
        to: u32,
        by: Actor,
        usage: Usage,
        fresh: Fresh,
    ) -> Result<(), Error> {
        let n = self.get(id).ok_or(Error::NotFound)?;
        if !self.controls(by, id) {
            return Err(Error::Denied);
        }
        if to != HOST {
            let t = self.get(to).ok_or(Error::NotFound)?;
            if let Actor::Cell(a) = by
                && a != to
                && !self.is_ancestor(a, to)
            {
                return Err(Error::Denied);
            }
            if to == id || self.is_ancestor(id, to) {
                return Err(Error::Cycle);
            }
            if t.domain != n.domain && by != Actor::Host {
                return Err(Error::Denied);
            }
        }
        if n.parent == to {
            return Ok(());
        }
        let sub = self.subtree(id)?;
        let depth = if to == HOST {
            1
        } else {
            self.node(to).depth + 1
        };
        if depth + self.height(&sub) > DEPTH_MAX {
            return Err(Error::TooDeep);
        }
        let charge = n.limits.charge();
        // Egress, ceiling and caps of the subtree under its new parent.
        let mut narrowed = Vec::new();
        if to != HOST {
            let room = self.free(to, usage(to));
            let p = self.node(to);
            if !p.limits.bounded() {
                return Err(Error::Unlimited);
            }
            if p.children >= CHILDREN_MAX {
                return Err(Error::TooManyChildren);
            }
            if !charge.fits(room) {
                return Err(Error::Budget);
            }
            let (egress, ceiling, cap) = (p.egress.clone(), p.ceiling.clone(), p.cpu_max);
            reserve(&mut narrowed, sub.len())?;
            for &c in &sub {
                let c = self.node(c);
                narrowed.push((
                    c.egress.intersect(&egress)?,
                    c.ceiling.intersect(&ceiling)?,
                    c.cpu_max.min(cap),
                ));
            }
        }
        let new_domain = if to == HOST {
            Some(fresh())
        } else if self.node(to).domain != n.domain {
            Some(self.node(to).domain)
        } else {
            None
        };
        // Nothing fails from here.
        let old = n.parent;
        if to == HOST && old != HOST && matches!(by, Actor::Cell(_)) {
            self.release(old, charge);
        }
        self.unlink(id);
        let shift = i16::from(depth) - i16::from(self.node(id).depth);
        for (k, &c) in sub.iter().enumerate() {
            let c = self.node_mut(c);
            c.depth = (i16::from(c.depth) + shift) as u8;
            if let Some((egress, ceiling, cap)) = narrowed.get(k) {
                (c.egress, c.ceiling, c.cpu_max) = (egress.clone(), ceiling.clone(), *cap);
            }
        }
        if to != HOST {
            self.link(id, to);
        }
        if let Some(domain) = new_domain {
            self.renumber(&sub, domain, to == HOST);
        }
        Ok(())
    }

    /// Takes `charge` out of the limits of `from` and every ancestor of it
    /// for good (a promotion at a cell's request, design D2), keeping each
    /// one's free budget.
    fn release(&mut self, from: u32, charge: Limits) {
        let mut at = from;
        while at != HOST {
            let n = self.node_mut(at);
            n.limits = n.limits.left(charge);
            at = n.parent;
        }
        let mut at = from;
        while at != HOST {
            let parent = self.node(at).parent;
            if parent != HOST {
                self.recount(parent);
            }
            at = parent;
        }
    }

    /// Puts the cells `sub` (a subtree, its top first) in domain `domain`
    /// and numbers them again there; with `fresh`, the domain is new and
    /// its top is its root.
    fn renumber(&mut self, sub: &[u32], domain: u32, fresh: bool) {
        if fresh {
            self.node_mut(sub[0]).next_pid = 1;
        }
        for &c in sub {
            self.node_mut(c).domain = domain;
        }
        for &c in sub {
            let pid = self.number(c, 0);
            self.node_mut(c).pid = pid;
        }
    }

    /// Ends cell `id` (design D3): each child whose policy is `Promote`
    /// becomes a root (its reservation leaving the surviving ancestors'
    /// limits, as a promotion at a cell's request), every other descendant
    /// ends with it, and the parent gets the reservation back. CPU time the
    /// cells used is spent for the whole subtree: the kernel counts it as
    /// the parent's own use ([`Ended::gone`] names whose).
    pub fn end(&mut self, id: u32, fresh: Fresh) -> Result<Ended, Error> {
        if self.get(id).is_none() {
            return Err(Error::NotFound);
        }
        // Who ends and who is promoted, before anything changes.
        let mut ended = Ended::default();
        reserve(&mut ended.gone, 1)?;
        ended.gone.push(id);
        let mut i = 0;
        while i < ended.gone.len() {
            let at = ended.gone[i];
            for c in self.children(at) {
                let list = match self.node(c).on_parent_end {
                    OnParentEnd::Cascade => &mut ended.gone,
                    OnParentEnd::Promote => &mut ended.promoted,
                };
                reserve(list, 1)?;
                list.push(c);
            }
            i += 1;
        }
        let mut subs = Vec::new();
        reserve(&mut subs, ended.promoted.len())?;
        for &p in &ended.promoted {
            subs.push(self.subtree(p)?);
        }
        let domains: Vec<u32> = {
            let mut d = Vec::new();
            reserve(&mut d, ended.promoted.len())?;
            d.extend(ended.promoted.iter().map(|_| fresh()));
            d
        };
        // Nothing fails from here.
        let parent = self.node(id).parent;
        for (k, &p) in ended.promoted.iter().enumerate() {
            let charge = self.node(p).limits.charge();
            if parent != HOST {
                self.release(parent, charge);
            }
            self.unlink(p);
            let shift = self.node(p).depth - 1;
            for &c in &subs[k] {
                self.node_mut(c).depth -= shift;
            }
            self.renumber(&subs[k], domains[k], true);
        }
        self.unlink(id);
        self.nodes.retain(|n| !ended.gone.contains(&n.id));
        ended.gone.reverse(); // children before parents
        Ok(ended)
    }

    /// Sets cell `id`'s limits at `by`'s request: the host sets a root's
    /// freely; any other change must fit the parent's free budget (as if
    /// the cell's old limits were given back), and no change may leave the
    /// cell less than its children's reservations and what it uses.
    pub fn set_limits(
        &mut self,
        id: u32,
        limits: Limits,
        by: Actor,
        usage: Usage,
    ) -> Result<(), Error> {
        let n = self.get(id).ok_or(Error::NotFound)?;
        if !self.controls(by, id) {
            return Err(Error::Denied);
        }
        if n.children > 0 && !limits.bounded() {
            return Err(Error::Unlimited);
        }
        if !n.reserved.add(usage(id)).fits(limits) {
            return Err(Error::Budget);
        }
        let parent = n.parent;
        if parent != HOST {
            let room = self.free(parent, usage(parent)).add(n.limits.charge());
            if !limits.charge().fits(room) {
                return Err(Error::Budget);
            }
        }
        self.node_mut(id).limits = limits;
        if parent != HOST {
            self.recount(parent);
        }
        Ok(())
    }

    /// Sets cell `id`'s egress policy and ceiling at `by`'s request (design
    /// D7): the host sets a root's freely and another cell's within its
    /// parent's; a cell may only narrow its descendants'. The descendants'
    /// become their intersection with the new ones.
    pub fn set_egress(
        &mut self,
        id: u32,
        egress: Egress,
        ceiling: Egress,
        by: Actor,
    ) -> Result<(), Error> {
        let n = self.get(id).ok_or(Error::NotFound)?;
        if !self.controls(by, id) {
            return Err(Error::Denied);
        }
        let allowed = match (by, n.parent) {
            (Actor::Host, HOST) => true,
            (Actor::Host, p) => {
                let p = self.node(p);
                egress.within(&p.egress) && ceiling.within(&p.ceiling)
            }
            (Actor::Cell(_), _) => egress.within(&n.egress) && ceiling.within(&n.ceiling),
        };
        if !allowed {
            return Err(Error::Denied);
        }
        let sub = self.subtree(id)?;
        let mut narrowed = Vec::new();
        reserve(&mut narrowed, sub.len())?;
        for &c in &sub[1..] {
            let c = self.node(c);
            narrowed.push((c.egress.intersect(&egress)?, c.ceiling.intersect(&ceiling)?));
        }
        for (&c, (e, ce)) in sub[1..].iter().zip(narrowed) {
            let c = self.node_mut(c);
            (c.egress, c.ceiling) = (e, ce);
        }
        let n = self.node_mut(id);
        (n.egress, n.ceiling) = (egress, ceiling);
        Ok(())
    }

    /// Sets what happens to cell `id` when its parent ends, at the request
    /// of an ancestor or the host (design D3).
    pub fn set_on_parent_end(
        &mut self,
        id: u32,
        policy: OnParentEnd,
        by: Actor,
    ) -> Result<(), Error> {
        if !self.controls(by, id) {
            return Err(if self.get(id).is_none() {
                Error::NotFound
            } else {
                Error::Denied
            });
        }
        self.node_mut(id).on_parent_end = policy;
        Ok(())
    }

    /// Checks the tree's invariants (design D19): identifiers in order;
    /// links, depths, domains and counts consistent; numbers unique in
    /// each domain and below its counter; every reservation the sum of
    /// its children's charges and within the limits; children only under
    /// bounded cells; policies and caps within the parent's.
    pub fn check(&self) -> Result<(), &'static str> {
        for w in self.nodes.windows(2) {
            if w[0].id >= w[1].id {
                return Err("identifiers out of order");
            }
        }
        for n in &self.nodes {
            if n.id == HOST || n.domain == 0 || n.pid == 0 {
                return Err("identifier, domain or number 0");
            }
            let root = self.root_of(n.id).ok_or("a broken parent chain")?;
            let r = self.node(root);
            if n.domain != r.domain || n.pid >= r.next_pid {
                return Err("domain or number unlike its root's");
            }
            if self
                .nodes
                .iter()
                .any(|m| m.id != n.id && m.domain == n.domain && m.pid == n.pid)
            {
                return Err("a number twice in a domain");
            }
            let count = self.children(n.id).count();
            if count != usize::from(n.children) || count > usize::from(CHILDREN_MAX) {
                return Err("children miscounted");
            }
            let mut sum = Limits::ZERO;
            for c in self.children(n.id) {
                let c = self.get(c).ok_or("a child not in the tree")?;
                if c.parent != n.id || c.depth != n.depth + 1 {
                    return Err("a child's parent or depth is wrong");
                }
                if !c.egress.within(&n.egress)
                    || !c.ceiling.within(&n.ceiling)
                    || c.cpu_max > n.cpu_max
                {
                    return Err("a child's policy is wider than its parent's");
                }
                sum = sum.add(c.limits.charge());
            }
            if sum != n.reserved {
                return Err("a reservation unlike its children's charges");
            }
            if !n.reserved.fits(n.limits) {
                return Err("reserved more than the limits");
            }
            if count > 0 && !n.limits.bounded() {
                return Err("children under unbounded limits");
            }
            if n.depth == 0 || n.depth > DEPTH_MAX || (n.is_root() != (n.depth == 1)) {
                return Err("a depth out of bounds");
            }
            if n.parent != HOST {
                let p = self.get(n.parent).ok_or("a parent not in the tree")?;
                if !self.children(p.id).any(|c| c == n.id) {
                    return Err("a cell missing from its parent's children");
                }
            } else if self
                .nodes
                .iter()
                .any(|m| m.is_root() && m.id != n.id && m.domain == n.domain)
            {
                return Err("two roots in a domain");
            }
        }
        Ok(())
    }
}
