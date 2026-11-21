//! The ownership tree (design D1 to D4 of `add-cell-hierarchy`).

use crate::limits::{CHILDREN_MAX, DEPTH_MAX, LABEL_MAX, WEIGHT_DEFAULT, WEIGHT_MAX, WEIGHT_MIN};
use crate::{Egress, Error, alloc_point, reserve};
use alloc::sync::Arc;
use alloc::vec::Vec;

/// The parent of a root: the host, which is not a cell.
pub const HOST: u32 = 0;

/// The number of a new domain's root and the next one, never `skip`.
fn first_numbers(skip: u32) -> (u32, u32) {
    if skip == 1 { (2, 3) } else { (1, 2) }
}

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

/// A cell's label: at most [`LABEL_MAX`] bytes of `[A-Za-z0-9._-]`,
/// shared by the copies of a cell (no allocation to copy one).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Label(Option<Arc<[u8]>>);

impl Label {
    /// The label `bytes`, or [`Error::Invalid`].
    pub fn new(bytes: &[u8]) -> Result<Self, Error> {
        let ok = |b: &u8| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-');
        if bytes.len() > LABEL_MAX || !bytes.iter().all(ok) {
            return Err(Error::Invalid);
        }
        if bytes.is_empty() {
            return Ok(Self(None));
        }
        alloc_point()?;
        Ok(Self(Some(Arc::from(bytes))))
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_deref().unwrap_or_default()
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

/// A cell's policies: egress (design D7), exposure ceiling (D8), label,
/// CPU weight and cap (D23) and exclusive CPU set (D17). Shared by the
/// cells that have the same ones (a cell's copies, every cell with the
/// defaults) and replaced, never changed, so a socket can keep the policy
/// it was made under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub egress: Egress,
    pub ceiling: Egress,
    pub label: Label,
    pub weight: u16,
    pub cpu_max: u32,
    pub cpus: u64,
}

impl Policy {
    /// What a cell gets when its creator decides nothing.
    const DEFAULT: Self = Self {
        egress: Egress::None,
        ceiling: Egress::None,
        label: Label(None),
        weight: WEIGHT_DEFAULT,
        cpu_max: u32::MAX,
        cpus: 0,
    };
}

/// One cell in the tree: a running or suspended cell, or a template. Small,
/// since the kernel keeps one per cell: its policies are shared, and what
/// its children reserved is summed from them when asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    id: u32,
    parent: u32,
    first_child: u32,
    next_sibling: u32,
    domain: u32,
    pid: u32,
    /// For a root: the number the next cell made in its domain gets.
    next_pid: u32,
    children: u16,
    depth: u8,
    on_parent_end: OnParentEnd,
    limits: Limits,
    policy: Arc<Policy>,
}

// The kernel keeps one node per cell (`cell.idle_bytes`), and copies one
// at each fork: it stays within a cache line.
const _: () = assert!(core::mem::size_of::<Node>() <= 64);

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
    pub fn mode(&self) -> Mode {
        Mode::Reserved
    }
    pub fn on_parent_end(&self) -> OnParentEnd {
        self.on_parent_end
    }
    /// What a creator would give a cell to make it as this one: its
    /// limits, its policy for when its parent ends and its policies (a
    /// snapshot keeps them, and a refused move puts them back).
    pub fn spec(&self) -> Spec {
        let p = &self.policy;
        Spec {
            limits: self.limits,
            on_parent_end: self.on_parent_end,
            egress: p.egress.clone(),
            ceiling: Some(p.ceiling.clone()),
            label: p.label.clone(),
            weight: p.weight,
            cpu_max: p.cpu_max,
            cpus: p.cpus,
        }
    }
    /// Its policies, shared and immutable.
    pub fn policy(&self) -> &Arc<Policy> {
        &self.policy
    }
    pub fn egress(&self) -> &Egress {
        &self.policy.egress
    }
    pub fn ceiling(&self) -> &Egress {
        &self.policy.ceiling
    }
    pub fn label(&self) -> &Label {
        &self.policy.label
    }
    pub fn weight(&self) -> u16 {
        self.policy.weight
    }
    pub fn cpu_max(&self) -> u32 {
        self.policy.cpu_max
    }
    pub fn cpus(&self) -> u64 {
        self.policy.cpus
    }
    /// How many children it has.
    pub fn children(&self) -> u16 {
        self.children
    }
}

/// What [`Tree::end`] did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ended {
    /// The descendants that ended with the cell (those whose policy was
    /// to cascade, below cells that ended), newest first.
    pub gone: Vec<u32>,
    /// The cells that became roots, with their subtrees (each in a new
    /// domain, numbered again).
    pub promoted: Vec<u32>,
}

/// The ownership tree of a kernel's cells (design D1), ordered by
/// identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    nodes: Vec<Node>,
    /// The default policies, shared by every cell that has them.
    default: Arc<Policy>,
}

impl Default for Tree {
    fn default() -> Self {
        Self::new()
    }
}

/// Fresh domain identifiers: never 0, never one a live domain has.
pub type Fresh<'a> = &'a mut dyn FnMut() -> u32;
/// What a cell uses itself, as limits (descendants: 0).
pub type Usage<'a> = &'a dyn Fn(u32) -> Limits;

impl Tree {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            default: Arc::new(Policy::DEFAULT),
        }
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

    /// What the children of cell `id` reserved: the sum of their charges.
    fn reserved_by(&self, id: u32) -> Limits {
        self.children(id)
            .fold(Limits::ZERO, |sum, c| sum.add(self.node(c).limits.charge()))
    }

    /// What cell `id`'s children reserved from its limits.
    pub fn reserved(&self, id: u32) -> Option<Limits> {
        self.get(id).map(|_| self.reserved_by(id))
    }

    /// What cell `id` itself may use: its limits less its children's
    /// reservations.
    pub fn own(&self, id: u32) -> Option<Limits> {
        self.get(id).map(|n| n.limits.left(self.reserved_by(id)))
    }

    /// The free budget of cell `id`, which uses `used` itself (design D4):
    /// its limits less what it uses and what its children reserved.
    pub fn free_budget(&self, id: u32, used: Limits) -> Option<Limits> {
        self.get(id).map(|_| self.free(id, used))
    }

    fn free(&self, id: u32, used: Limits) -> Limits {
        self.node(id).limits.left(self.reserved_by(id)).left(used)
    }

    /// Adds root `id` of a new domain, whose identifier is `id` (the
    /// kernel's identifiers are fresh: never one a domain had), and returns
    /// its number: 1, or 2 when `skip` is 1 (`skip` is the source's number
    /// for a copy, so a copy's `getpid` differs, design D22; 0 for none).
    pub fn add_root(&mut self, id: u32, spec: Spec, skip: u32) -> Result<u32, Error> {
        if id == HOST {
            return Err(Error::Invalid);
        }
        let mut node = self.made(id, HOST, id, spec)?;
        let i = self.insert_at(id)?;
        (node.pid, node.next_pid) = first_numbers(skip);
        self.nodes.insert(i, node);
        Ok(self.nodes[i].pid)
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
        if !spec.egress.within(p.egress())
            || !ceiling.within(p.ceiling())
            || spec.cpu_max > p.cpu_max()
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
        let policy = Policy {
            egress: spec.egress,
            ceiling,
            label: spec.label,
            weight: spec.weight,
            cpu_max: spec.cpu_max,
            cpus: spec.cpus,
        };
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
            domain,
            pid: 0,
            next_pid: 1,
            children: 0,
            depth,
            on_parent_end: spec.on_parent_end,
            limits: spec.limits,
            policy: self.shared(policy)?,
        })
    }

    /// `policy`, shared with the default when it is the default.
    fn shared(&self, policy: Policy) -> Result<Arc<Policy>, Error> {
        if policy == *self.default {
            return Ok(self.default.clone());
        }
        alloc_point()?;
        Ok(Arc::new(policy))
    }

    /// `node`'s policies with egress `egress` and ceiling `ceiling`: the
    /// same record when they do not change.
    fn narrowed(
        &self,
        node: &Node,
        egress: Egress,
        ceiling: Egress,
        cap: u32,
    ) -> Result<Arc<Policy>, Error> {
        let p = &node.policy;
        if p.egress == egress && p.ceiling == ceiling && p.cpu_max == cap {
            return Ok(p.clone());
        }
        self.shared(Policy {
            egress,
            ceiling,
            cpu_max: cap,
            ..Policy::clone(p)
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
    /// (its identifier the copy's), numbered from 1 but never with the
    /// source's number;
    /// copies of any other cell are carved from its parent's budget again.
    /// A copy has the source's limits and policies, and no children.
    /// `placed(k, pid, domain)` learns the number and domain of copy `k`.
    pub fn add_copies(
        &mut self,
        source: u32,
        ids: &[u32],
        usage: Usage,
        placed: &mut dyn FnMut(usize, u32, u32),
    ) -> Result<(), Error> {
        let si = self.index(source).ok_or(Error::NotFound)?;
        let s = &self.nodes[si];
        let (parent, pid) = (s.parent, s.pid);
        let count = u32::try_from(ids.len()).map_err(|_| Error::Invalid)?;
        if parent != HOST {
            self.room_for(parent, s.limits.charge(), count, usage)?;
        }
        // Where a single copy goes, found once.
        let mut place = None;
        for (k, &id) in ids.iter().enumerate() {
            let at = self.nodes.binary_search_by_key(&id, |n| n.id);
            if id == HOST || ids[..k].contains(&id) || at.is_ok() {
                return Err(Error::Exists);
            }
            place = at.err().filter(|_| ids.len() == 1);
        }
        reserve(&mut self.nodes, ids.len())?;
        // An exclusive CPU set stays with one domain.
        let set = self.nodes[si].policy.cpus != 0;
        let policy = match set {
            true => Some(self.shared(Policy {
                cpus: 0,
                ..Policy::clone(&self.nodes[si].policy)
            })?),
            false => None,
        };
        // Nothing fails from here. Each copy is made in its place.
        let mut si = si;
        for (k, &id) in ids.iter().enumerate() {
            let i = place.unwrap_or_else(|| {
                let at = self.nodes.binary_search_by_key(&id, |n| n.id);
                at.unwrap_err()
            });
            let node = self.nodes[si].clone();
            self.nodes.insert(i, node);
            si += usize::from(i <= si);
            let n = &mut self.nodes[i];
            (n.id, n.first_child, n.next_sibling, n.children) = (id, HOST, HOST, 0);
            if let Some(p) = &policy {
                n.policy = p.clone();
            }
            if parent == HOST {
                n.domain = id;
                (n.pid, n.next_pid) = first_numbers(pid);
                placed(k, n.pid, id);
            } else {
                let domain = n.domain;
                self.link(id, parent);
                let p = self.number(id, 0);
                self.node_mut(id).pid = p;
                placed(k, p, domain);
            }
        }
        Ok(())
    }

    /// Gives cell `old` the identifier `new` (a resume gives the cell a new
    /// kernel identifier); its place and domain stay. A resume is a copy
    /// (design D22): the cell takes a new number in its domain, never its
    /// old one. Returns its node, or `None` if `old` was not in the tree or
    /// `new` was.
    pub fn rekey(&mut self, old: u32, new: u32) -> Option<&Node> {
        let at = self.nodes.binary_search_by_key(&new, |n| n.id).err()?;
        if new == HOST {
            return None;
        }
        let i = self.index(old)?;
        // Moves the node to its new place in one rotation.
        let at = if at > i {
            self.nodes[i..at].rotate_left(1);
            at - 1
        } else {
            self.nodes[at..=i].rotate_right(1);
            at
        };
        let node = &mut self.nodes[at];
        node.id = new;
        let (parent, first) = (node.parent, node.first_child);
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
        let pid = self.number(new, 0);
        self.nodes[at].pid = pid;
        Some(&self.nodes[at])
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
            let top = p.policy.clone();
            reserve(&mut narrowed, sub.len())?;
            for &c in &sub {
                let c = self.node(c);
                let egress = c.egress().intersect(&top.egress)?;
                let ceiling = c.ceiling().intersect(&top.ceiling)?;
                narrowed.push(self.narrowed(c, egress, ceiling, c.cpu_max().min(top.cpu_max))?);
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
            if let Some(policy) = narrowed.get(k) {
                c.policy = policy.clone();
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
        let n = self.get(id).ok_or(Error::NotFound)?;
        if n.children == 0 {
            self.unlink(id);
            let i = self.index(id).expect("a cell of the tree");
            self.nodes.remove(i);
            return Ok(Ended::default());
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
        ended.gone.swap_remove(0); // the cell itself
        ended.gone.sort_unstable_by(|a, b| b.cmp(a));
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
        if !self.reserved_by(id).add(usage(id)).fits(limits) {
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
                egress.within(p.egress()) && ceiling.within(p.ceiling())
            }
            (Actor::Cell(_), _) => egress.within(n.egress()) && ceiling.within(n.ceiling()),
        };
        if !allowed {
            return Err(Error::Denied);
        }
        let sub = self.subtree(id)?;
        let mut narrowed = Vec::new();
        reserve(&mut narrowed, sub.len())?;
        for &c in &sub[1..] {
            let c = self.node(c);
            let (e, ce) = (
                c.egress().intersect(&egress)?,
                c.ceiling().intersect(&ceiling)?,
            );
            narrowed.push(self.narrowed(c, e, ce, c.cpu_max())?);
        }
        let top = self.node(id);
        let policy = self.narrowed(top, egress, ceiling, top.cpu_max())?;
        for (&c, policy) in sub[1..].iter().zip(narrowed) {
            self.node_mut(c).policy = policy;
        }
        self.node_mut(id).policy = policy;
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
                if !c.egress().within(n.egress())
                    || !c.ceiling().within(n.ceiling())
                    || c.cpu_max() > n.cpu_max()
                {
                    return Err("a child's policy is wider than its parent's");
                }
                sum = sum.add(c.limits.charge());
            }
            if !sum.fits(n.limits) {
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
