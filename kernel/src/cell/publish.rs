//! Content shared across domains (design D21 of `add-cell-hierarchy`):
//! templates the host publishes, copies into other domains that share only
//! public objects, and cells that leave their domain.

use super::*;

/// What the host published, and the memory of dropped templates that other
/// domains may still share.
#[derive(Debug, Default)]
pub(super) struct Published {
    /// Templates published at some time, with the file pages their
    /// publication pins (`None` once withdrawn: copies made before may
    /// still be in other domains).
    templates: Vec<(u32, Option<Vec<rhizo_fs::Page>>)>,
    /// The tables of templates that were published and are dropped, held
    /// until no other cell shares their memory (item 12 of design D12).
    held: Vec<u64>,
}

/// Why [`Host::publish`] refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// There is no such template.
    NoTemplate,
    /// The cell it was made of may hold a secret (it reached the network
    /// card), and the host did not attest that it holds none (design D22).
    MayHoldSecret,
}

/// A copy of `pt`'s user half that shares only public frames (design D21
/// of `add-cell-hierarchy`), its frames counted by `meter`; `None` if they
/// do not fit it or memory.
pub(super) fn full_copy(pt: &PageTable<Format>, meter: &Meter) -> Option<PageTable<Format>> {
    pt.copy_lower_half(
        &mut DirectMap,
        &mut cell_frames(meter),
        &mut public_frames(),
    )
    .ok()
}

/// Bytes `pt`'s user half maps.
pub(super) fn mapped_bytes(pt: &PageTable<Format>) -> u64 {
    let mut bytes = 0;
    pt.for_each_lower_leaf(&DirectMap, &mut |_, _, size, _| bytes += size.bytes());
    bytes
}

impl Host {
    /// Publishes template `id` (spec `control-plane`, publishing
    /// templates): its copies may then go into any domain sharing its
    /// frames, its files are public, and the pages of their executables
    /// and libraries are loaded now and kept until it is withdrawn and
    /// unmapped (design D21). Refused for a template whose cell reached the
    /// network card, unless the host attests (`attested`) that it holds no
    /// secret (design D22). Publishing it again changes nothing.
    #[cfg_attr(
        not(any(feature = "ktest", feature = "bench-cell")),
        allow(
            dead_code,
            reason = "used by in-kernel tests and benchmarks until protocol 4 (task 1.11)"
        )
    )]
    pub fn publish(&mut self, id: u32, attested: bool) -> Result<(), Refused> {
        match self.may_publish(id, attested) {
            None => return Err(Refused::NoTemplate),
            Some(false) => return Err(Refused::MayHoldSecret),
            Some(true) => {}
        }
        if self.is_published(id) {
            return Ok(());
        }
        let Some((_, fs, _)) = self.templates.iter_mut().find(|(c, ..)| c.id == id) else {
            return Err(Refused::NoTemplate);
        };
        let pinned = publish_files(fs);
        let templates = &mut self.published.templates;
        match templates.iter_mut().find(|(t, _)| *t == id) {
            Some((_, pages)) => *pages = Some(pinned), // withdrawn before
            None => templates.push((id, Some(pinned))),
        }
        Ok(())
    }

    /// Withdraws template `id` (spec `control-plane`): later copies outside
    /// its domain are full copies, and its public pages go once unmapped;
    /// copies made before keep sharing, and its memory stays until the
    /// last of them ends. Returns whether it was published.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(
            dead_code,
            reason = "used by in-kernel tests until protocol 4 (task 1.11)"
        )
    )]
    pub fn withdraw(&mut self, id: u32) -> bool {
        let found = self.published.templates.iter_mut().find(|(t, _)| *t == id);
        match found.and_then(|(_, pages)| pages.take()) {
            Some(pages) => {
                withdraw_pages(&pages);
                true
            }
            None => false,
        }
    }

    /// Whether template `id` is published now.
    pub(super) fn is_published(&self, id: u32) -> bool {
        let found = self.published.templates.iter().find(|(t, _)| *t == id);
        found.is_some_and(|(_, pages)| pages.is_some())
    }

    /// Whether copies of cell `id` go into a domain of their own: those of
    /// a root, which are roots (design D13), or of a cell not in the tree.
    pub(super) fn copies_leave_domain(&self, id: u32) -> bool {
        self.tree.get(id).is_none_or(|n| n.is_root())
    }

    /// The memory of template `id`, whose tables are at `root`, goes: at
    /// once, unless it was ever published and another cell still shares
    /// it, which then keeps it until none does (item 12 of design D12: no
    /// copy in one domain is ever the sole owner of a frame another domain
    /// shared, so no copy's write cost tells another copy's end).
    pub(super) fn end_template_memory(&mut self, id: u32, root: u64) {
        self.withdraw(id);
        let pt = PageTable::<Format>::from_root(root);
        let published = self.published.templates.iter().position(|(t, _)| *t == id);
        if let Some(at) = published {
            self.published.templates.swap_remove(at);
            if sharing(true, || still_shared(&pt)) {
                self.published.held.push(root);
                return;
            }
        }
        destroy_shared(pt);
    }

    /// Frees the memory of the dropped templates held that no cell shares
    /// any more: called whenever cells end.
    pub(super) fn release_held(&mut self) {
        if self.published.held.is_empty() {
            return;
        }
        self.published.held.retain(|&root| {
            let pt = PageTable::<Format>::from_root(root);
            let held = sharing(true, || still_shared(&pt));
            if !held {
                destroy_shared(pt);
            }
            held
        });
    }

    /// Frees every held template's memory, whatever still shares it (the
    /// host goes).
    pub(super) fn drop_held(&mut self) {
        for root in core::mem::take(&mut self.published.held) {
            let pt = PageTable::<Format>::from_root(root);
            destroy_shared(pt);
        }
    }

    /// Gives cell `id` (live or a template) private copies of every page it
    /// shares, public objects' excepted, before it leaves its domain
    /// (design D21): it then shares nothing with its old domain. `false`,
    /// changing nothing, if its memory quota or the machine cannot hold
    /// them. A suspended cell's snapshot moves when it is resumed.
    pub(super) fn unshare(&mut self, id: u32) -> bool {
        if let Some((cell, ..)) = self.templates.iter_mut().find(|(c, ..)| c.id == id) {
            let old = PageTable::<Format>::from_root(cell.root);
            if !shares_private(&old, true) {
                return true;
            }
            if !cells_may_take(mapped_bytes(&old)) {
                return false;
            }
            let Some(new) = sharing(true, || full_copy(&old, &Meter::new(0, u64::MAX))) else {
                return false;
            };
            cell.root = new.root();
            destroy_shared(old);
            return true;
        }
        let Some((mut live, wake)) = self.take_cell(id) else {
            return true; // suspended, or gone
        };
        let old = PageTable::<Format>::from_root(live.cell.root);
        if let Some(lazy) = live.demand.lazy.as_mut() {
            lazy.finish(&old, &live.frames);
        }
        live.demand.lazy = None;
        if !shares_private(&old, live.shares) {
            self.requeue(live, wake);
            return true;
        }
        let used = Meter::new(0, live.frames.limit());
        let copy = match cells_may_take(mapped_bytes(&old)) {
            true => sharing(live.shares, || full_copy(&old, &used)),
            false => None,
        };
        let Some(new) = copy else {
            self.requeue(live, wake);
            return false;
        };
        live.cell.root = new.root();
        // Its old translations go before its old frames do.
        user::flush_user(live.cell.root, live.tag);
        live.clean = flushed_by_kernel_on(crate::arch::cpu::index());
        sharing(live.shares, || {
            space::destroy(old, &mut DirectMap, &mut KernelFrames)
        });
        live.frames.set_used(used.used());
        live.shares = false;
        self.requeue(live, wake);
        true
    }
}

/// Whether another cell may reach a page of `pt` that is no public
/// object's, under [`SHARING`] if `locked`.
fn shares_private(pt: &PageTable<Format>, locked: bool) -> bool {
    sharing(locked, || {
        pt.shares(&DirectMap, &KernelFrames, &mut public_frames())
    })
}

/// Whether another cell may still reach a page of `pt` that no cache of
/// the kernel holds (a page a cache holds never tells a cell another's
/// end). Under [`SHARING`].
fn still_shared(pt: &PageTable<Format>) -> bool {
    pt.shares(&DirectMap, &KernelFrames, &mut kept_frames())
}

/// Frees `pt`, whose tables and pages other cells may share.
fn destroy_shared(pt: PageTable<Format>) {
    sharing(true, || {
        space::destroy(pt, &mut DirectMap, &mut KernelFrames)
    });
}
