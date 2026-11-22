//! Handles as grants (design D8, D9 and D24 of `add-cell-hierarchy`): who
//! holds which kernel object with which rights, who granted it, what it was
//! derived from, and the exposure check every new flow edge passes.
//!
//! Every handle is a [`Grant`]. A grant made from another (an attenuation,
//! a transfer, a copy's inheritance) is its child, so the grants form a
//! derivation forest and revoking one removes its whole subtree. Grant
//! numbers are indices of the table, private to the kernel: a cell only
//! ever sees descriptors and `/run/rhizo` entries the kernel maps to them.
//!
//! Every operation checks and allocates before it changes anything, so a
//! refused or failed one leaves the table as it was (design D19).
//! Revocation, unlinking and the end of a cell never allocate and never
//! fail.

use crate::limits::{CHAIN_MAX, CHANNELS_MAX, DEPTH_MAX, HANDLES_MAX};
use crate::{Actor, Channels, Egress, Error, HOST, Label, Tree, reserve};
use alloc::vec::Vec;

/// What a handle allows on its object (design D9). Each kind of object has
/// its own rights; a grant holds a subset of its kind's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rights(u16);

impl Rights {
    pub const NONE: Self = Self(0);
    /// Cell: its state changes and output.
    pub const OBSERVE: Self = Self(1);
    /// Cell: write its standard input.
    pub const INPUT: Self = Self(1 << 1);
    /// Cell: open a channel to it.
    pub const MESSAGE: Self = Self(1 << 2);
    /// Cell: the operations of design D2 (on a template: make copies).
    pub const CONTROL: Self = Self(1 << 3);
    /// Channel endpoint: send messages to the peer.
    pub const SEND: Self = Self(1 << 4);
    /// Channel endpoint: receive the messages queued on it.
    pub const RECEIVE: Self = Self(1 << 5);
    /// Channel endpoint: move handles with messages (`SCM_RIGHTS`).
    pub const CARRY: Self = Self(1 << 6);
    /// Accelerator: model sessions on the models the handle lists.
    pub const INFER: Self = Self(1 << 7);
    /// Accelerator: compute sessions.
    pub const COMPUTE: Self = Self(1 << 8);
    /// Accelerator: its subtree's use.
    pub const STATS: Self = Self(1 << 9);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    /// Whether every right of `other` is here.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
    /// Whether any right of `other` is here.
    pub const fn any(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

impl core::ops::BitOr for Rights {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        self.union(other)
    }
}

/// The kernel object a handle names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Object {
    /// A cell (or a template) by its identifier in the tree.
    Cell(u32),
    /// An endpoint of a channel of [`Channels`]: channel `e >> 1`, side
    /// `e & 1`; its peer is `e ^ 1`.
    Endpoint(u32),
    /// An accelerator the host registered ([`Grants::register_accel`]).
    Accel(u32),
}

impl Object {
    /// Every right a handle on this kind of object may hold.
    pub const fn rights(self) -> Rights {
        match self {
            Object::Cell(_) => Rights::OBSERVE
                .union(Rights::INPUT)
                .union(Rights::MESSAGE)
                .union(Rights::CONTROL),
            Object::Endpoint(_) => Rights::SEND.union(Rights::RECEIVE).union(Rights::CARRY),
            Object::Accel(_) => Rights::INFER.union(Rights::COMPUTE).union(Rights::STATS),
        }
    }
}

/// The accelerator budgets of `add-accelerators` D5 (design D24): device
/// memory and KV bytes in host memory in bytes, device time in
/// milliseconds per second, open sessions. Reserved only, like the tree's
/// limits: a derived handle's are carved from its source's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AccelBudget {
    pub mem: u64,
    pub kv_host: u64,
    pub time: u32,
    pub sessions: u32,
}

impl AccelBudget {
    pub const ZERO: Self = Self {
        mem: 0,
        kv_host: 0,
        time: 0,
        sessions: 0,
    };

    /// `self + b`, or `None` past the fields' range.
    fn add(self, b: Self) -> Option<Self> {
        Some(Self {
            mem: self.mem.checked_add(b.mem)?,
            kv_host: self.kv_host.checked_add(b.kv_host)?,
            time: self.time.checked_add(b.time)?,
            sessions: self.sessions.checked_add(b.sessions)?,
        })
    }

    /// `self - b`, or `None` if any field would go negative.
    fn left(self, b: Self) -> Option<Self> {
        Some(Self {
            mem: self.mem.checked_sub(b.mem)?,
            kv_host: self.kv_host.checked_sub(b.kv_host)?,
            time: self.time.checked_sub(b.time)?,
            sessions: self.sessions.checked_sub(b.sessions)?,
        })
    }

    /// Whether every field is at most `room`'s.
    pub fn fits(self, room: Self) -> bool {
        room.left(self).is_some()
    }
}

/// What an accelerator handle allows beyond its rights: the models it may
/// open sessions on, one bit per model the host registered (at most
/// [`MODELS_MAX`](crate::limits::MODELS_MAX)), and its budget. Zero for
/// handles on other objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AccelTerms {
    pub models: u64,
    pub budget: AccelBudget,
}

impl AccelTerms {
    /// Whether these terms allow nothing `wide` does not.
    pub fn within(&self, wide: &AccelTerms) -> bool {
        self.models & !wide.models == 0 && self.budget.fits(wide.budget)
    }
}

/// What one asks a new grant to be: its rights, its holder, the label the
/// granter chose for it in the holder's `/run/rhizo`, and, for an
/// accelerator, its terms.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ask {
    pub rights: Rights,
    pub holder: u32,
    pub label: Label,
    pub accel: AccelTerms,
}

/// No grant.
const NIL: u32 = u32::MAX;

/// One handle: `{object, rights, holder, grantor, parent grant}` (design
/// D9), with its place in the derivation forest. Everything a snapshot
/// records of a handle (kind, rights, label, terms) is plain data here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    object: Object,
    rights: Rights,
    /// Length of its derivation chain: 1 for a grant made from no other.
    depth: u8,
    holder: u32,
    /// Who made it: [`HOST`], or the cell that granted, derived or sent it.
    grantor: u32,
    parent: u32,
    first_child: u32,
    next_sibling: u32,
    label: Label,
    accel: AccelTerms,
}

impl Grant {
    pub fn object(&self) -> Object {
        self.object
    }
    pub fn rights(&self) -> Rights {
        self.rights
    }
    /// The cell that holds it.
    pub fn holder(&self) -> u32 {
        self.holder
    }
    /// The cell that made it, [`HOST`] for the host.
    pub fn grantor(&self) -> u32 {
        self.grantor
    }
    /// The grant it was derived from.
    pub fn parent(&self) -> Option<u32> {
        (self.parent != NIL).then_some(self.parent)
    }
    /// Length of its derivation chain, 1 for a grant made from no other.
    pub fn chain(&self) -> u8 {
        self.depth
    }
    pub fn label(&self) -> &Label {
        &self.label
    }
    /// Its accelerator terms (zero for other objects).
    pub fn accel(&self) -> &AccelTerms {
        &self.accel
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Slot {
    Used(Grant),
    /// A free slot, and the next free one.
    Free(u32),
}

/// What a cell uses of an accelerator handle's budget itself (its own
/// sessions), by grant number.
pub type AccelUse<'a> = &'a dyn Fn(u32) -> AccelBudget;
/// Told each grant that leaves the table, with its number.
pub type Gone<'a> = &'a mut dyn FnMut(u32, &Grant);

/// The grant table of a kernel (design D9), and the accelerators the host
/// registered (D24).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grants {
    slots: Vec<Slot>,
    /// The first free slot, then each one's next.
    free: u32,
    /// Registered accelerators: identifier, models and capacity.
    accels: Vec<(u32, AccelTerms)>,
}

impl Default for Grants {
    fn default() -> Self {
        Self::new()
    }
}

/// The flow rights of a cell handle, from the holder to the cell and back
/// (design D8).
const TO_CELL: Rights = Rights::INPUT.union(Rights::CONTROL);
const FROM_CELL: Rights = Rights::OBSERVE.union(Rights::CONTROL);

impl Grants {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: NIL,
            accels: Vec::new(),
        }
    }

    /// Grant `id`.
    pub fn get(&self, id: u32) -> Option<&Grant> {
        match self.slots.get(id as usize) {
            Some(Slot::Used(g)) => Some(g),
            _ => None,
        }
    }

    fn used(&self, id: u32) -> &Grant {
        self.get(id).expect("a grant of the table")
    }

    fn used_mut(&mut self, id: u32) -> &mut Grant {
        match &mut self.slots[id as usize] {
            Slot::Used(g) => g,
            Slot::Free(_) => panic!("a grant of the table"),
        }
    }

    /// Every grant with its number.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &Grant)> {
        self.slots.iter().enumerate().filter_map(|(i, s)| match s {
            Slot::Used(g) => Some((i as u32, g)),
            Slot::Free(_) => None,
        })
    }

    /// How many handles and how many channel endpoints `cell` holds.
    fn counts(&self, cell: u32) -> (u32, u32) {
        self.iter()
            .filter(|(_, g)| g.holder == cell)
            .fold((0, 0), |(h, c), (_, g)| match g.object {
                Object::Endpoint(_) => (h, c + 1),
                _ => (h + 1, c),
            })
    }

    /// The registered terms of accelerator `id`.
    fn accel(&self, id: u32) -> Option<&AccelTerms> {
        self.accels.iter().find(|a| a.0 == id).map(|a| &a.1)
    }

    /// Registers accelerator `id` (the host, design D24): the models it
    /// serves and the capacity the broker bound for it, against which the
    /// host's own grants of it are admitted.
    pub fn register_accel(&mut self, id: u32, terms: AccelTerms) -> Result<(), Error> {
        if self.accel(id).is_some() {
            return Err(Error::Exists);
        }
        reserve(&mut self.accels, 1)?;
        self.accels.push((id, terms));
        Ok(())
    }

    /// Removes accelerator `id` and every handle on it.
    pub fn unregister_accel(&mut self, id: u32, gone: Gone) {
        self.accels.retain(|a| a.0 != id);
        self.object_gone(Object::Accel(id), gone);
    }

    /// The checks every new grant passes, whoever makes it: rights of its
    /// kind, a live holder and object, control never delivered to the
    /// cell it names or a descendant (design D2), its chain and its
    /// holder's handles within bounds (D19).
    fn valid(&self, tree: &Tree, g: &Grant) -> Result<(), Error> {
        if !g.object.rights().contains(g.rights) {
            return Err(Error::Invalid);
        }
        if !matches!(g.object, Object::Accel(_)) && g.accel != AccelTerms::default() {
            return Err(Error::Invalid);
        }
        if tree.get(g.holder).is_none() {
            return Err(Error::NotFound);
        }
        if let Object::Cell(x) = g.object {
            if tree.get(x).is_none() {
                return Err(Error::NotFound);
            }
            if g.rights.any(Rights::CONTROL) && (g.holder == x || tree.is_ancestor(x, g.holder)) {
                return Err(Error::Denied);
            }
        }
        if u32::from(g.depth) > CHAIN_MAX {
            return Err(Error::TooDeep);
        }
        let (handles, channels) = self.counts(g.holder);
        let full = match g.object {
            Object::Endpoint(_) => channels >= CHANNELS_MAX,
            _ => handles >= HANDLES_MAX,
        };
        if full {
            return Err(Error::TooManyHandles);
        }
        Ok(())
    }

    /// A grant from `ask`, on `object`, by `grantor`, under `parent`.
    fn made(object: Object, ask: Ask, grantor: u32, parent: Option<&Grant>, at: u32) -> Grant {
        Grant {
            object,
            rights: ask.rights,
            depth: parent.map_or(1, |p| p.depth.saturating_add(1)),
            holder: ask.holder,
            grantor,
            parent: if parent.is_some() { at } else { NIL },
            first_child: NIL,
            next_sibling: NIL,
            label: ask.label,
            accel: ask.accel,
        }
    }

    /// Room for `n` more grants; nothing fails from here to their insertion.
    fn room(&mut self, n: usize) -> Result<(), Error> {
        reserve(&mut self.slots, n)
    }

    /// Puts `g` in a slot (room made) and links it under its parent.
    fn insert(&mut self, mut g: Grant) -> u32 {
        let parent = g.parent;
        if parent != NIL {
            g.next_sibling = self.used(parent).first_child;
        }
        let id = if self.free != NIL {
            let id = self.free;
            let Slot::Free(next) = self.slots[id as usize] else {
                unreachable!("the free list holds free slots");
            };
            self.free = next;
            self.slots[id as usize] = Slot::Used(g);
            id
        } else {
            self.slots.push(Slot::Used(g));
            (self.slots.len() - 1) as u32
        };
        if parent != NIL {
            self.used_mut(parent).first_child = id;
        }
        id
    }

    /// Frees slot `id` and returns its grant (links left as they were).
    fn release(&mut self, id: u32) -> Grant {
        let slot = core::mem::replace(&mut self.slots[id as usize], Slot::Free(self.free));
        self.free = id;
        match slot {
            Slot::Used(g) => g,
            Slot::Free(_) => unreachable!("a grant of the table"),
        }
    }

    /// Checks `g` (with the exposure check) and adds it.
    fn add(&mut self, tree: &Tree, g: Grant) -> Result<u32, Error> {
        self.valid(tree, &g)?;
        self.room(1)?;
        let edge = [Rec::of(&g)];
        self.exposed(tree, &[g.holder], &edge)?;
        Ok(self.insert(g))
    }

    /// Makes a grant derived from no other, at `by`'s request (design D9):
    /// the host grants any object; a cell grants only to itself or a
    /// descendant, a handle on a descendant or an endpoint of a channel it
    /// made; only the host grants accelerators, within what it registered
    /// (design D24). Refused if a flow edge it adds breaks a cell's
    /// exposure ceiling (design D8).
    pub fn grant(
        &mut self,
        tree: &Tree,
        channels: &Channels,
        by: Actor,
        object: Object,
        ask: Ask,
    ) -> Result<u32, Error> {
        let grantor = match by {
            Actor::Host => HOST,
            Actor::Cell(a) => a,
        };
        if let Object::Endpoint(e) = object {
            let owner = channels.owner(e >> 1).ok_or(Error::NotFound)?;
            if by != Actor::Host && owner != grantor {
                return Err(Error::Denied);
            }
        }
        if let Actor::Cell(a) = by {
            let mine = |c: u32| c == a || tree.is_ancestor(a, c);
            match object {
                Object::Cell(x) if tree.get(x).is_some() && !tree.is_ancestor(a, x) => {
                    return Err(Error::Denied);
                }
                Object::Accel(_) => return Err(Error::Denied),
                _ => {}
            }
            if tree.get(ask.holder).is_some() && !mine(ask.holder) {
                return Err(Error::Denied);
            }
        }
        if let Object::Accel(id) = object {
            let terms = self.accel(id).ok_or(Error::NotFound)?;
            if ask.accel.models & !terms.models != 0 {
                return Err(Error::Denied);
            }
            let granted = self
                .iter()
                .filter(|(_, g)| g.object == object && g.parent == NIL)
                .try_fold(ask.accel.budget, |sum, (_, g)| sum.add(g.accel.budget));
            if !granted.is_some_and(|b| b.fits(terms.budget)) {
                return Err(Error::Budget);
            }
        }
        self.add(tree, Self::made(object, ask, grantor, None, NIL))
    }

    /// The child of grant `from` that `ask` describes, by its holder `by`:
    /// at most its rights and, for an accelerator, at most its models and
    /// a budget carved from what it has free (its budget less its
    /// children's and `usage(from)`).
    fn derived(&self, by: u32, from: u32, ask: Ask, usage: AccelUse) -> Result<Grant, Error> {
        let src = self.get(from).ok_or(Error::NotFound)?;
        if src.holder != by {
            return Err(Error::Denied);
        }
        if !src.rights.contains(ask.rights) {
            return Err(Error::Denied);
        }
        if let Object::Accel(_) = src.object {
            if ask.accel.models & !src.accel.models != 0 {
                return Err(Error::Denied);
            }
            let free = self
                .children_of(from)
                .try_fold(usage(from), |sum, c| sum.add(self.used(c).accel.budget))
                .and_then(|taken| src.accel.budget.left(taken));
            if !free.is_some_and(|f| ask.accel.budget.fits(f)) {
                return Err(Error::Budget);
            }
        }
        Ok(Self::made(src.object, ask, by, Some(src), from))
    }

    /// The grants derived directly from `id`, newest first.
    fn children_of(&self, id: u32) -> impl Iterator<Item = u32> + '_ {
        let mut at = self.used(id).first_child;
        core::iter::from_fn(move || {
            (at != NIL).then(|| {
                let id = at;
                at = self.used(id).next_sibling;
                id
            })
        })
    }

    /// Attenuation (design D9): cell `by` derives from its grant `from` a
    /// grant with at most its rights (models, budget), held by itself or
    /// given to a descendant. No request yields more than `from` holds.
    pub fn derive(
        &mut self,
        tree: &Tree,
        by: u32,
        from: u32,
        ask: Ask,
        usage: AccelUse,
    ) -> Result<u32, Error> {
        if ask.holder != by && !tree.is_ancestor(by, ask.holder) {
            return Err(Error::Denied);
        }
        let g = self.derived(by, from, ask, usage)?;
        self.add(tree, g)
    }

    /// Transfer (design D9): cell `by` sends its grant `from` over its
    /// endpoint grant `over`, which needs the carry right, to `ask.holder`,
    /// a holder of the peer endpoint. The receiver gets a grant derived
    /// from `from`, with `by` as grantor, so `by` can revoke it.
    pub fn transfer(
        &mut self,
        tree: &Tree,
        by: u32,
        from: u32,
        over: u32,
        ask: Ask,
        usage: AccelUse,
    ) -> Result<u32, Error> {
        let o = self.get(over).ok_or(Error::NotFound)?;
        let Object::Endpoint(e) = o.object else {
            return Err(Error::Invalid);
        };
        if o.holder != by || !o.rights.contains(Rights::CARRY) {
            return Err(Error::Denied);
        }
        let peer = Object::Endpoint(e ^ 1);
        if !self
            .iter()
            .any(|(_, g)| g.object == peer && g.holder == ask.holder)
        {
            return Err(Error::Denied);
        }
        let g = self.derived(by, from, ask, usage)?;
        self.add(tree, g)
    }

    /// A message carrying grant `id` (made by [`transfer`](Self::transfer)
    /// for one holder of the endpoint it was sent to) is taken by `holder`,
    /// another holder of that endpoint `on`: `id` becomes `holder`'s, with
    /// the checks every new grant passes (bounds, control never flowing up,
    /// the exposure check). Refused, changing nothing, if `holder` holds no
    /// grant on `on` with the receive right.
    pub fn rehome(&mut self, tree: &Tree, id: u32, holder: u32, on: Object) -> Result<(), Error> {
        let g = self.get(id).ok_or(Error::NotFound)?;
        if g.holder == holder {
            return Ok(());
        }
        if !self.iter().any(|(_, r)| {
            r.object == on && r.holder == holder && r.rights.contains(Rights::RECEIVE)
        }) {
            return Err(Error::Denied);
        }
        let mut moved = g.clone();
        moved.holder = holder;
        self.valid(tree, &moved)?;
        // The check runs with the old holder's edge still in the table:
        // removing an edge only narrows what anyone reaches.
        self.exposed(tree, &[holder], &[Rec::of(&moved)])?;
        self.used_mut(id).holder = holder;
        Ok(())
    }

    /// Unlinks grant `id` from its parent's children.
    fn detach(&mut self, id: u32) {
        let (parent, next) = {
            let g = self.used(id);
            (g.parent, g.next_sibling)
        };
        if parent == NIL {
            return;
        }
        if self.used(parent).first_child == id {
            self.used_mut(parent).first_child = next;
        } else {
            let mut at = self.used(parent).first_child;
            while self.used(at).next_sibling != id {
                at = self.used(at).next_sibling;
            }
            self.used_mut(at).next_sibling = next;
        }
    }

    /// The deepest first descendant of `id`, where a walk in post-order
    /// starts.
    fn first_leaf(&self, mut id: u32) -> u32 {
        while self.used(id).first_child != NIL {
            id = self.used(id).first_child;
        }
        id
    }

    /// Removes grant `id` with its derivation subtree, in post-order and
    /// without allocating: at a cost proportional to its size.
    fn remove_tree(&mut self, id: u32, gone: Gone) {
        self.detach(id);
        let mut at = self.first_leaf(id);
        loop {
            let (next, parent) = {
                let g = self.used(at);
                (g.next_sibling, g.parent)
            };
            let last = at == id;
            let then = match (last, next) {
                (true, _) => NIL,
                (false, NIL) => parent,
                (false, s) => self.first_leaf(s),
            };
            let g = self.release(at);
            gone(at, &g);
            if last {
                return;
            }
            at = then;
        }
    }

    /// Revokes grant `id` and everything derived from it, at `by`'s request
    /// (design D9): the host any grant, a cell those whose grantor is
    /// itself or a cell of its subtree.
    pub fn revoke(&mut self, tree: &Tree, by: Actor, id: u32, gone: Gone) -> Result<(), Error> {
        let g = self.get(id).ok_or(Error::NotFound)?;
        if let Actor::Cell(a) = by
            && g.grantor != a
            && (g.grantor == HOST || !tree.is_ancestor(a, g.grantor))
        {
            return Err(Error::Denied);
        }
        self.remove_tree(id, gone);
        Ok(())
    }

    /// Revokes every grant cell `by` derived from its grant `id`, which it
    /// keeps (`revoke(fd)`, design D9).
    pub fn revoke_derived(&mut self, by: u32, id: u32, gone: Gone) -> Result<(), Error> {
        if self.get(id).ok_or(Error::NotFound)?.holder != by {
            return Err(Error::Denied);
        }
        loop {
            let c = self.used(id).first_child;
            if c == NIL {
                return Ok(());
            }
            self.remove_tree(c, gone);
        }
    }

    /// Cell `by` drops its grant `id` (`unlink`, a close): what was derived
    /// from it stays, now derived from its parent, so whoever could revoke
    /// it still can.
    pub fn unlink(&mut self, by: u32, id: u32) -> Result<Grant, Error> {
        if self.get(id).ok_or(Error::NotFound)?.holder != by {
            return Err(Error::Denied);
        }
        Ok(self.drop_one(id))
    }

    fn drop_one(&mut self, id: u32) -> Grant {
        self.detach(id);
        let (parent, first) = {
            let g = self.used(id);
            (g.parent, g.first_child)
        };
        let mut c = first;
        let mut last = NIL;
        while c != NIL {
            self.shorten(c);
            let g = self.used_mut(c);
            g.parent = parent;
            last = c;
            c = g.next_sibling;
            if parent == NIL {
                g.next_sibling = NIL;
            }
        }
        if parent != NIL && last != NIL {
            let head = self.used(parent).first_child;
            self.used_mut(last).next_sibling = head;
            self.used_mut(parent).first_child = first;
        }
        self.release(id)
    }

    /// Shortens by one the chains of grant `top` and its subtree (its
    /// parent leaves the chain), in pre-order without allocating.
    fn shorten(&mut self, top: u32) {
        let mut at = top;
        loop {
            self.used_mut(at).depth -= 1;
            let c = self.used(at).first_child;
            if c != NIL {
                at = c;
                continue;
            }
            loop {
                if at == top {
                    return;
                }
                let s = self.used(at).next_sibling;
                if s != NIL {
                    at = s;
                    break;
                }
                at = self.used(at).parent;
            }
        }
    }

    /// Removes every grant on `object` (it ended). They form whole
    /// derivation trees, since a derived grant names its source's object.
    pub fn object_gone(&mut self, object: Object, gone: Gone) {
        for id in 0..self.slots.len() as u32 {
            if self.get(id).is_some_and(|g| g.object == object) {
                let g = self.release(id);
                gone(id, &g);
            }
        }
    }

    /// Cell `id` left the tree: the handles on it are revoked and those it
    /// held dropped (what was derived from them stays, as after
    /// [`unlink`](Self::unlink)).
    pub fn cell_gone(&mut self, id: u32, gone: Gone) {
        self.object_gone(Object::Cell(id), gone);
        for g in 0..self.slots.len() as u32 {
            if self.get(g).is_some_and(|x| x.holder == id) {
                let x = self.drop_one(g);
                gone(g, &x);
            }
        }
    }

    /// Gives the cells `copies`, just made as copies of `source` (design
    /// D13, D24), the handles they inherit: for each of the source's cell
    /// handles, a grant derived from it with the same grantor, rights and
    /// label, all or none. A copy inherits no accelerator handle (D24), no
    /// control handle on the source's descendants (D13) and no channel
    /// endpoint: an endpoint whose peer is outside the copy reads as reset
    /// there, and a channel within the source is copied as a new one with
    /// its messages, which its own grants then name.
    pub fn inherit(&mut self, tree: &Tree, source: u32, copies: &[u32]) -> Result<(), Error> {
        if tree.get(source).is_none() {
            return Err(Error::NotFound);
        }
        for &c in copies {
            if tree.get(c).is_none() {
                return Err(Error::NotFound);
            }
            if self.iter().any(|(_, g)| g.holder == c) {
                return Err(Error::Exists);
            }
        }
        let kept = |g: &Grant| match g.object {
            Object::Cell(x) => !(g.rights.any(Rights::CONTROL) && tree.is_ancestor(source, x)),
            Object::Endpoint(_) | Object::Accel(_) => false,
        };
        let mut new: Vec<(u32, Grant)> = Vec::new();
        let mut edges: Vec<Rec> = Vec::new();
        let n = self
            .iter()
            .filter(|(_, g)| g.holder == source && kept(g))
            .count();
        reserve(&mut new, n * copies.len())?;
        reserve(&mut edges, n * copies.len())?;
        for (id, g) in self.iter().filter(|(_, g)| g.holder == source && kept(g)) {
            for &c in copies {
                let ask = Ask {
                    rights: g.rights,
                    holder: c,
                    label: g.label.clone(),
                    accel: AccelTerms::default(),
                };
                let copy = Self::made(g.object, ask, g.grantor, Some(g), id);
                self.valid(tree, &copy)?;
                edges.push(Rec::of(&copy));
                new.push((id, copy));
            }
        }
        self.room(new.len())?;
        self.exposed(tree, copies, &edges)?;
        for (_, g) in new {
            self.insert(g);
        }
        Ok(())
    }

    /// Whether the grants hold under `tree` around the cells `cells` (after
    /// a change the kernel made to the tree, such as a move, design D8):
    /// no control handle on a cell held by it or a descendant, and every
    /// exposure that could reach them within its ceiling. `Denied` if not.
    pub fn admits(&self, tree: &Tree, cells: &[u32]) -> Result<(), Error> {
        for (_, g) in self.iter() {
            if let Object::Cell(x) = g.object
                && g.rights.any(Rights::CONTROL)
                && (g.holder == x || tree.is_ancestor(x, g.holder))
            {
                return Err(Error::Denied);
            }
        }
        self.exposed(tree, cells, &[])
    }

    /// The exposure check (design D8) with the grants `extra` added, for
    /// every cell that could reach one of `starts`: the cells it reaches
    /// along flow edges, without entering one of its ancestors, have
    /// egress within its ceiling. Only cells connected to `starts` can have
    /// a reach that changed, so only they are checked.
    fn exposed(&self, tree: &Tree, starts: &[u32], extra: &[Rec]) -> Result<(), Error> {
        let flows = Flows::new(tree, self, extra)?;
        let near = flows.reach(starts, None)?;
        // The policies the connected cells have, each once: a cell whose
        // ceiling holds all of them needs no walk of its own.
        let mut policies: Vec<&Egress> = Vec::new();
        for &c in &near {
            let e = tree.get(c).expect("a cell of the tree").egress();
            if !policies.contains(&e) {
                reserve(&mut policies, 1)?;
                policies.push(e);
            }
        }
        for &c in &near {
            let ceiling = tree.get(c).expect("a cell of the tree").ceiling();
            if policies.iter().all(|e| e.within(ceiling)) {
                continue;
            }
            for r in flows.reach(&[c], Some(c))? {
                if !tree
                    .get(r)
                    .expect("a cell of the tree")
                    .egress()
                    .within(ceiling)
                {
                    return Err(Error::Denied);
                }
            }
        }
        Ok(())
    }

    /// Checks the table's invariants (design D19) under `tree` and
    /// `channels`: links, chains and the free list consistent; every
    /// holder and object live; rights of their kind, a derived grant's
    /// rights and terms within its parent's, children's budgets within
    /// their parent's and the host's grants within what it registered;
    /// handles within their bounds; control never flowing up; every
    /// exposure within its ceiling.
    pub fn check(&self, tree: &Tree, channels: &Channels) -> Result<(), &'static str> {
        let mut free = 0;
        let mut at = self.free;
        while at != NIL {
            match self.slots.get(at as usize) {
                Some(Slot::Free(next)) => at = *next,
                _ => return Err("a broken free list"),
            }
            free += 1;
            if free > self.slots.len() {
                return Err("a free list in a loop");
            }
        }
        if free != self.slots.len() - self.iter().count() {
            return Err("free slots off the free list");
        }
        for (id, g) in self.iter() {
            if !g.object.rights().contains(g.rights) {
                return Err("a right of another kind");
            }
            if tree.get(g.holder).is_none() {
                return Err("a holder not in the tree");
            }
            let live = match g.object {
                Object::Cell(x) => tree.get(x).is_some(),
                Object::Endpoint(e) => channels.owner(e >> 1).is_some(),
                Object::Accel(a) => self.accel(a).is_some(),
            };
            if !live {
                return Err("a handle on an object that ended");
            }
            if let Object::Cell(x) = g.object
                && g.rights.any(Rights::CONTROL)
                && (g.holder == x || tree.is_ancestor(x, g.holder))
            {
                return Err("control flowing up");
            }
            if g.depth == 0 || u32::from(g.depth) > CHAIN_MAX {
                return Err("a chain out of bounds");
            }
            let mut sum = Some(AccelBudget::ZERO);
            for c in self.children_of(id) {
                let c = self.get(c).ok_or("a child not in the table")?;
                if c.parent != id || c.depth != g.depth + 1 || c.object != g.object {
                    return Err("a child's parent, chain or object is wrong");
                }
                if !g.rights.contains(c.rights) || c.accel.models & !g.accel.models != 0 {
                    return Err("a derived grant wider than its source");
                }
                sum = sum.and_then(|s| s.add(c.accel.budget));
            }
            if !sum.is_some_and(|s| s.fits(g.accel.budget)) {
                return Err("children's budgets over their source's");
            }
            if g.parent == NIL {
                if g.depth != 1 {
                    return Err("a chain without a parent");
                }
            } else if !self.children_of(g.parent).any(|c| c == id) {
                return Err("a grant missing from its parent's children");
            }
            let (handles, endpoints) = self.counts(g.holder);
            if handles > HANDLES_MAX || endpoints > CHANNELS_MAX {
                return Err("handles over their bounds");
            }
        }
        for (a, terms) in &self.accels {
            let granted = self
                .iter()
                .filter(|(_, g)| g.object == Object::Accel(*a) && g.parent == NIL)
                .try_fold(AccelBudget::ZERO, |s, (_, g)| {
                    (g.accel.models & !terms.models == 0)
                        .then_some(())
                        .and_then(|_| s.add(g.accel.budget))
                });
            if !granted.is_some_and(|s| s.fits(terms.budget)) {
                return Err("the host's grants over what it registered");
            }
        }
        let cells: Vec<u32> = tree.iter().map(|n| n.id()).collect();
        if self.exposed(tree, &cells, &[]).is_err() {
            return Err("an exposure over its ceiling");
        }
        Ok(())
    }
}

/// What the exposure check needs of a grant.
#[derive(Debug, Clone, Copy)]
struct Rec {
    holder: u32,
    object: Object,
    rights: Rights,
}

impl Rec {
    fn of(g: &Grant) -> Self {
        Self {
            holder: g.holder,
            object: g.object,
            rights: g.rights,
        }
    }
}

/// The flow edges of design D8, from the tree and the grants: indexed by
/// holder and by object, built once per check.
struct Flows<'a> {
    tree: &'a Tree,
    by_holder: Vec<Rec>,
    by_object: Vec<Rec>,
}

impl<'a> Flows<'a> {
    fn new(tree: &'a Tree, grants: &Grants, extra: &[Rec]) -> Result<Self, Error> {
        let n = grants.iter().count() + extra.len();
        let mut by_holder = Vec::new();
        reserve(&mut by_holder, n)?;
        by_holder.extend(grants.iter().map(|(_, g)| Rec::of(g)));
        by_holder.extend_from_slice(extra);
        let mut by_object = Vec::new();
        reserve(&mut by_object, n)?;
        by_object.extend_from_slice(&by_holder);
        by_holder.sort_unstable_by_key(|r| r.holder);
        by_object.sort_unstable_by_key(|r| r.object);
        Ok(Self {
            tree,
            by_holder,
            by_object,
        })
    }

    fn held(&self, cell: u32) -> &[Rec] {
        let lo = self.by_holder.partition_point(|r| r.holder < cell);
        let hi = self.by_holder.partition_point(|r| r.holder <= cell);
        &self.by_holder[lo..hi]
    }

    fn on(&self, object: Object) -> &[Rec] {
        let lo = self.by_object.partition_point(|r| r.object < object);
        let hi = self.by_object.partition_point(|r| r.object <= object);
        &self.by_object[lo..hi]
    }

    /// The cells information can flow to from `x` (`directed`), or that it
    /// shares an edge with either way. Holders of one endpoint share its
    /// queue, so they are joined both ways; a sending holder reaches the
    /// peer's holders; holders of one accelerator are joined (D24).
    fn next(
        &self,
        x: u32,
        directed: bool,
        f: &mut dyn FnMut(u32) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let (to, from) = match directed {
            true => (TO_CELL, FROM_CELL),
            false => (TO_CELL.union(FROM_CELL), TO_CELL.union(FROM_CELL)),
        };
        let node = self.tree.get(x).expect("a cell of the tree");
        if !node.is_root() {
            f(node.parent())?;
        }
        for c in self.tree.children(x) {
            f(c)?;
        }
        for r in self.held(x) {
            match r.object {
                Object::Endpoint(e) => {
                    for s in self.on(r.object) {
                        f(s.holder)?;
                    }
                    if !directed || r.rights.any(Rights::SEND) {
                        for s in self.on(Object::Endpoint(e ^ 1)) {
                            f(s.holder)?;
                        }
                    }
                }
                Object::Cell(y) if r.rights.any(to) => f(y)?,
                Object::Cell(_) => {}
                Object::Accel(_) => {
                    for s in self.on(r.object) {
                        f(s.holder)?;
                    }
                }
            }
        }
        for r in self.on(Object::Cell(x)) {
            if r.rights.any(from) {
                f(r.holder)?;
            }
        }
        Ok(())
    }

    /// The cells reached from `starts`: along flow edges without entering
    /// an ancestor of `from` when given, along edges either way otherwise.
    fn reach(&self, starts: &[u32], from: Option<u32>) -> Result<Vec<u32>, Error> {
        let mut ancestors = [HOST; DEPTH_MAX as usize];
        let mut n = 0;
        if let Some(c) = from {
            let mut at = self.tree.get(c).expect("a cell of the tree").parent();
            while at != HOST {
                ancestors[n] = at;
                n += 1;
                at = self.tree.get(at).expect("a cell of the tree").parent();
            }
        }
        let ancestors = &ancestors[..n];
        let (mut order, mut seen) = (Vec::new(), Vec::new());
        let mut visit = |order: &mut Vec<u32>, y: u32| -> Result<(), Error> {
            if ancestors.contains(&y) {
                return Ok(());
            }
            if let Err(at) = seen.binary_search(&y) {
                reserve(&mut seen, 1)?;
                reserve(order, 1)?;
                seen.insert(at, y);
                order.push(y);
            }
            Ok(())
        };
        for &s in starts {
            visit(&mut order, s)?;
        }
        let mut i = 0;
        while let Some(&x) = order.get(i) {
            self.next(x, from.is_some(), &mut |y| visit(&mut order, y))?;
            i += 1;
        }
        Ok(order)
    }
}
