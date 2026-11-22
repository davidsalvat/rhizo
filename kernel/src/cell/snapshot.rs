//! Suspending, resuming and forking cells, and templates (specs
//! `snapshots` and `fork`).

use super::*;
use rhizo_snapshot::UNKNOWN_GENERATION;

/// How [`Host::resume`] restores memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    /// Every page before the cell runs.
    Eager,
    /// Each page the first time it is touched.
    Lazy,
}

/// Readies `snapshot`, decoded from a file and capped to what a cell the
/// host starts gets, for a placement under a parent (design D13 of
/// `add-cell-hierarchy`): it enters the tree as a root before the host
/// places it, and there it holds a new cell's limits, never those its bytes
/// claim. Its placement then carves its limits from the parent's budget.
/// Without this, a file claiming a larger limit than the kernel can set
/// aside could not be placed under any parent, however small the limits
/// the host gives it there.
#[cfg_attr(
    not(any(feature = "control", feature = "ktest")),
    allow(dead_code, reason = "used by the control kernel and in-kernel tests")
)]
pub fn to_place(snapshot: &mut Snapshot) {
    let files = snapshot.state().quota.files;
    snapshot.set_quota(Quota {
        files,
        ..Quota::DEFAULT
    });
    let spec = rhizo_cap::Spec {
        limits: rhizo_cap::Limits::UNLIMITED,
        ..snapshot.tree().spec
    };
    snapshot.set_tree(&spec);
}

impl Host {
    /// Suspends live cell `id` into a snapshot and frees all its memory
    /// (spec `snapshots`). Its captured output stays with the host.
    pub fn suspend(&mut self, id: u32) -> Option<Snapshot> {
        let _p = cost::scope(Part::Spawn);
        // The snapshot lives in the kernel heap, and pages still pending from
        // a lazy resume need frames first: refuse what would not fit
        // (design D2 of `add-memory-accounting`); the cell keeps running.
        let (mut live, wake) = self.take_cell(id)?;
        let mut bytes = 0;
        PageTable::<Format>::from_root(live.cell.root)
            .for_each_lower_leaf(&DirectMap, &mut |_, _, size, _| bytes += size.bytes());
        if let Some(lazy) = &live.demand.lazy {
            bytes += 2 * lazy.pending_bytes();
        }
        // What its memory limit leaves is its own (design D4 of
        // `add-cell-hierarchy`): its snapshot stays charged to it.
        if !may_take(&live.frames, bytes) {
            self.requeue(live, wake);
            return None;
        }
        let pt = PageTable::<Format>::from_root(live.cell.root);
        if let Some(lazy) = live.demand.lazy.as_mut()
            && !lazy.finish(&pt, &live.frames)
        {
            self.requeue(live, wake);
            return None;
        }
        sync_fp(&mut live, true);
        let (files, sockets) = export_sockets(&live.fs);
        // Refers to the pages it shares instead of copying them (design D1
        // of `add-shared-snapshots`); its tables become its own first, under
        // the lock of shared tables when it has some.
        let mut snapshot = sharing(live.shares, || {
            Snapshot::capture::<Format>(
                user::MACHINE,
                &live.cell,
                &live.deadlines,
                &live.demand.wx,
                &files,
                &sockets,
                now_ns(),
                &mut DirectMap,
                &mut KernelFrames,
                release_frame,
            )
        });
        self.record_tree(&mut snapshot, id);
        // Files never changed: its resume shares them again without
        // comparing their bytes.
        if files.shares_with(&no_files()) {
            snapshot.mark_plain_files();
        }
        // What its limit leaves stays set aside for it (design D4).
        let (used, limit) = (live.frames.used(), live.frames.limit());
        self.end(live, Ending::Suspended);
        if limit < Quota::NONE.frames() {
            let frames = limit.saturating_sub(used);
            memory::RESERVE.hold(frames);
            self.held_aside.push((id, frames));
        }
        Some(snapshot)
    }

    /// Resumes `snapshot` as a new live cell (new identifier and tag) that
    /// continues exactly where the snapshot was taken: its threads waiting
    /// on descriptors make their calls again with the time they had left
    /// (design D5 of `snapshots-on-disk`). Gives the snapshot back if
    /// memory cannot hold the cell (eagerly: all its pages).
    pub fn resume(
        &mut self,
        snapshot: Snapshot,
        mode: Resume,
        capture: bool,
        echo: bool,
    ) -> Result<u32, Snapshot> {
        let _p = cost::scope(Part::Spawn);
        // A cell suspended here keeps its node, and its network's policy
        // (design D7); a loaded one starts with none, unless placed.
        let origin = snapshot.origin();
        let policy = has_policies() && self.suspended.contains(&origin);
        let live = self.restore(snapshot, mode, capture, echo)?;
        if policy {
            resume_network(origin, live.cell.id);
        }
        let id = live.cell.id;
        // New to the scheduler: `file` lets its threads waiting on
        // descriptors make their calls again.
        self.admit(|s| file(s, None, live));
        Ok(id)
    }

    /// Makes `snapshot` a template (spec `fork`), restored eagerly and never
    /// run (design D3 of `snapshots-on-disk`); returns its identifier, or
    /// gives the snapshot back if memory cannot hold it.
    #[cfg_attr(
        not(any(feature = "control", feature = "ktest")),
        allow(dead_code, reason = "used by the control kernel and in-kernel tests")
    )]
    pub fn load_template(&mut self, snapshot: Snapshot) -> Result<u32, Snapshot> {
        let _p = cost::scope(Part::Spawn);
        let live = self.restore(snapshot, Resume::Eager, false, false)?;
        if live.tag != 0 {
            self.locked(|s| s.tags.free(live.tag));
        }
        let Live {
            cell, fs, demand, ..
        } = *live;
        let id = cell.id;
        self.templates.push((cell, fs, demand.wx));
        Ok(id)
    }

    /// The live cell `snapshot` holds, not yet scheduled.
    fn restore(
        &mut self,
        snapshot: Snapshot,
        mode: Resume,
        capture: bool,
        echo: bool,
    ) -> Result<Box<Live>, Snapshot> {
        if mode == Resume::Eager {
            let bytes = (0..snapshot.pages())
                .map(|i| snapshot.page(i))
                .filter(|p| matches!(p.data, Data::Bytes(_)))
                .map(|p| p.size.bytes())
                .sum::<u64>();
            // A suspended cell's reservation, set aside, is its own.
            let origin = snapshot.origin();
            let aside = self
                .held_aside
                .iter()
                .find(|&&(c, _)| c == origin)
                .map_or(0, |&(_, frames)| frames * FRAME_SIZE);
            if !cells_may_take(bytes.saturating_sub(aside)) {
                return Err(snapshot);
            }
        }
        let Ok(pt) = PageTable::<Format>::new(&mut DirectMap, &mut || KernelFrames.alloc()) else {
            return Err(snapshot);
        };
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        if user::CELL_ROOT_HAS_KERNEL_HALF {
            pt.share_upper_half(&mut DirectMap, &user::cell_kernel_half());
        }
        let now = now_ns();
        let mut cell = snapshot.cell(id, pt.root(), now);
        // A resume or a load is a copy (design D22 of `add-cell-hierarchy`);
        // the snapshot holds its wipe ranges as zeros. Each load of bytes,
        // which may be loaded many times, draws its own generation.
        cell.generation = if snapshot.origin() == 0 || cell.generation == UNKNOWN_GENERATION {
            loaded_generation(cell.generation)
        } else {
            cell.generation + 1
        };
        if !self.enter_restored(&mut cell, &snapshot) {
            space::destroy(pt, &mut DirectMap, &mut KernelFrames);
            return Err(snapshot);
        }
        let mut lazy = Lazy::new(snapshot);
        // Resumed into another domain than it was taken in (a suspended
        // cell moved meanwhile, a copy of a template's snapshot), it first
        // stops referring to its old domain's frames (design D21 of
        // `add-cell-hierarchy`). One comparison otherwise, the snapshot
        // where it is.
        if cell.domain != lazy.snapshot().domain() && !self.moved(lazy.snapshot_mut(), &cell) {
            space::destroy(pt, &mut DirectMap, &mut KernelFrames);
            return Err(lazy.into_snapshot());
        }
        let snapshot = lazy.snapshot();
        let deadlines = snapshot.deadlines(now);
        let mut fs = files_of(snapshot);
        if snapshot.has_sockets() {
            import_sockets(&mut fs, &snapshot.sockets(), id);
        }
        let wx = snapshot.wx();
        let used = Meter::new(1, u64::MAX); // the root
        if mode == Resume::Eager {
            // Short of frames, what is left stays pending, restored lazily.
            let _ = lazy.finish(&pt, &used);
        }
        let mut live = self.new_live(cell, capture, echo, fs, used.used());
        live.demand.lazy = (lazy.left > 0).then_some(lazy);
        live.demand.wx = wx;
        live.deadlines = deadlines;
        Ok(live)
    }

    /// Forks live cell `id` into `n` children that share its memory
    /// copy-on-write (spec `fork`); returns their identifiers. A child has
    /// the cell's state, a new identifier and tag, and its own output.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn fork(&mut self, id: u32, n: usize) -> Option<Vec<u32>> {
        let _p = cost::scope(Part::Spawn);
        let (mut parent, wake) = self.take_cell(id)?;
        let limited = parent.cell.quota.memory != u64::MAX;
        if limited && !self.admits_copies(id, n) {
            self.requeue(parent, wake);
            return None;
        }
        let pt = PageTable::<Format>::from_root(parent.cell.root);
        // Pages still pending from a lazy resume come back first; short of
        // frames, no copy is made and the cell keeps the rest pending.
        if let Some(lazy) = parent.demand.lazy.as_mut()
            && !lazy.finish(&pt, &parent.frames)
        {
            self.requeue(parent, wake);
            return None;
        }
        parent.demand.lazy = None;
        sync_fp(&mut parent, false);
        let mut children = Vec::with_capacity(n);
        for _ in 0..n {
            let Some(child) = self.child_of(
                parent.cell.clone(),
                parent.fs.clone(),
                parent.demand.wx.clone(),
                parent.output.is_some(),
                parent.echo,
            ) else {
                break;
            };
            children.push(child);
        }
        // The parent lost write access to everything it shares: drop what its
        // TLB still caches (on other CPUs, when it next runs there).
        user::flush_user(parent.cell.root, parent.tag);
        parent.clean = flushed_by_kernel_on(crate::arch::cpu::index());
        parent.shares = true;
        self.requeue(parent, wake);
        if children.len() < n || !self.enter_copies(id, &mut children) {
            // All or nothing: the children made so far never ran.
            for child in &children {
                self.discard(child);
            }
            return None;
        }
        if limited {
            self.copies_entered(id);
        }
        let ids: Vec<u32> = children.iter().map(|c| c.cell.id).collect();
        // Copies have their source's egress policy (design D7).
        self.enter_networks(ids.iter().copied());
        for child in children {
            self.requeue(child, wake);
        }
        Some(ids)
    }

    /// Frees the memory, tag and sockets of a cell that never ran, leaving
    /// no trace (no [`Finished`]).
    pub(super) fn discard(&mut self, live: &Live) {
        if live.tag != 0 {
            self.locked(|s| s.tags.free(live.tag));
        }
        release_sockets(&live.fs, live.cell.id, true);
        sharing(true, || {
            space::destroy(
                PageTable::<Format>::from_root(live.cell.root),
                &mut DirectMap,
                &mut KernelFrames,
            )
        });
    }

    /// A new live cell sharing `cell`'s memory copy-on-write, with its
    /// files `fs` and its write-or-execute ranges `wx`. Only counts go up,
    /// on tables `cell` holds, and only `cell`'s root changes: no
    /// [`SHARING`], even while cells sharing those tables run (design D3 of
    /// `host-ops-per-cell`). It stays in `cell`'s domain (design D13 of
    /// `add-cell-hierarchy`), so sharing tells it nothing it does not hold.
    /// It is a copy (design D22 of
    /// `add-cell-hierarchy`): its generation is one more, and its wipe
    /// ranges read as zero, the only work done for cells that have some.
    fn child_of(
        &mut self,
        cell: Cell,
        fs: Fs,
        wx: WxRanges,
        capture: bool,
        echo: bool,
    ) -> Option<Box<Live>> {
        let pt = PageTable::<Format>::from_root(cell.root);
        let child = pt.fork_lower_half(&mut DirectMap, &mut KernelFrames).ok()?;
        // The child has the cell's quotas and none of its usage (design D3
        // of `add-cell-quotas`): what it shares stays counted to the cell.
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let cell = Cell {
            id,
            root: child.root(),
            cpu_ns: 0,
            generation: cell.generation + 1,
            ..cell
        };
        let mut live = self.new_live(cell, capture, echo, fork_files(fs, id), 1); // its root
        live.shares = true;
        let wiped = !wx.wiped().is_empty();
        live.demand.wx = wx;
        if wiped && !wipe_copy(&live) {
            self.discard(&live);
            return None;
        }
        Some(live)
    }

    /// Puts back a cell taken out of the scheduler: asleep until `wake`, or
    /// runnable.
    pub(super) fn requeue(&mut self, live: Box<Live>, wake: Option<u64>) {
        self.admit(|s| match wake {
            Some(at) => s.sched.sleep(live, at),
            None => {
                s.sched.place(live);
            }
        });
    }

    /// Freezes live cell `id` into a template: it never runs again, and
    /// [`spawn_from_template`](Self::spawn_from_template) forks it.
    #[cfg_attr(
        not(any(feature = "ktest", feature = "bench-cell", feature = "control")),
        allow(
            dead_code,
            reason = "used by in-kernel tests, benchmarks and the control kernel"
        )
    )]
    pub fn freeze(&mut self, id: u32) -> bool {
        let _p = cost::scope(Part::Spawn);
        let Some((mut live, wake)) = self.take_cell(id) else {
            return false;
        };
        let pt = PageTable::<Format>::from_root(live.cell.root);
        if let Some(lazy) = live.demand.lazy.as_mut()
            && !lazy.finish(&pt, &live.frames)
        {
            self.requeue(live, wake);
            return false;
        }
        sync_fp(&mut live, true);
        user::release_user(live.cell.root, live.tag);
        if live.tag != 0 {
            self.locked(|s| s.tags.free(live.tag));
        }
        let Live {
            cell, fs, demand, ..
        } = *live;
        self.templates.push((cell, fs, demand.wx));
        true
    }

    /// A snapshot of template `id` (design D3 of `snapshots-on-disk`): it
    /// refers to the template's pages it shares, which stay the
    /// template's. Its sockets are recorded, none parked. `None` when the
    /// kernel heap could not hold it and its export.
    #[cfg_attr(
        not(any(feature = "control", feature = "ktest")),
        allow(dead_code, reason = "used by the control kernel and in-kernel tests")
    )]
    pub fn template_snapshot(&mut self, id: u32) -> Option<Snapshot> {
        let _p = cost::scope(Part::Spawn);
        let (cell, fs, wx) = self.templates.iter().find(|(c, ..)| c.id == id)?;
        let mut bytes = 0;
        PageTable::<Format>::from_root(cell.root)
            .for_each_lower_leaf(&DirectMap, &mut |_, _, size, _| bytes += size.bytes());
        if !cells_may_take(2 * bytes) {
            return None;
        }
        let (files, sockets) = export_sockets(fs);
        let mut snapshot = sharing(true, || {
            Snapshot::capture::<Format>(
                user::MACHINE,
                cell,
                &[],
                wx,
                &files,
                &sockets,
                now_ns(),
                &mut DirectMap,
                &mut KernelFrames,
                release_frame,
            )
        });
        self.record_tree(&mut snapshot, id);
        close_parked(&snapshot);
        Some(snapshot)
    }

    /// Whether template `id` may be published, its copies then going into
    /// other domains (design D22 of `add-cell-hierarchy`): not if the cell
    /// it was made of reached the network card, unless the host attests
    /// (`attested`) that it holds no secret. `None` if there is no such
    /// template ([`publish`](Self::publish) asks it). A refusal, and a
    /// publication on the host's word, are audited (design D18).
    #[cfg_attr(
        not(any(feature = "ktest", feature = "control")),
        allow(dead_code, reason = "used by in-kernel tests and the control kernel")
    )]
    pub fn may_publish(&self, id: u32, attested: bool) -> Option<bool> {
        let (cell, ..) = self.templates.iter().find(|(c, ..)| c.id == id)?;
        if cell.exposed {
            let domain = self.tree.get().root_of(id).unwrap_or(0);
            if attested {
                crate::audit::record(
                    crate::audit::Kind::Attested,
                    id,
                    domain,
                    b"published on the host's word that it holds no secret: it reached the \
                      network card",
                );
            } else {
                crate::audit::deny(
                    crate::audit::Kind::Publish,
                    id,
                    domain,
                    b"it reached the network card: publishing it needs the host's attestation",
                );
            }
        }
        Some(attested || !cell.exposed)
    }

    /// Drops template `id`: its memory and sockets go back once no copy
    /// shares them. Returns whether it existed.
    pub fn drop_template(&mut self, id: u32) -> bool {
        if !self.remove_template(id) {
            return false;
        }
        self.ends.push(id);
        self.settle();
        true
    }

    /// Drops template `id` without applying it to the tree; returns whether
    /// it existed.
    pub(super) fn remove_template(&mut self, id: u32) -> bool {
        let Some(index) = self.templates.iter().position(|(c, ..)| c.id == id) else {
            return false;
        };
        let (template, fs, _) = self.templates.swap_remove(index);
        self.end_template_memory(template.id, template.root);
        release_sockets(&fs, template.id, true);
        self.cell_left(template.id, Ending::Killed);
        true
    }

    /// Starts a new cell from template `id`: a fork of the frozen cell, with
    /// every deadline of its threads expired (spec `fork`, templates keep no
    /// time).
    #[cfg_attr(
        not(any(feature = "ktest", feature = "bench-cell", feature = "control")),
        allow(
            dead_code,
            reason = "used by in-kernel tests, benchmarks and the control kernel"
        )
    )]
    pub fn spawn_from_template(&mut self, id: u32, capture: bool, echo: bool) -> Option<u32> {
        let _p = cost::scope(Part::Spawn);
        let index = self.templates.iter().position(|(c, ..)| c.id == id)?;
        let limited = self.templates[index].0.quota.memory != u64::MAX;
        if limited && !self.admits_copies(id, 1) {
            return None;
        }
        let (cell, fs, wx) = &self.templates[index];
        let (cell, fs, wx) = (cell.clone(), fs.clone(), wx.clone());
        let mut child = self.child_of(cell, fs, wx, capture, echo)?;
        child.expire_timers();
        let mut copies = [child];
        if !self.enter_copies(id, &mut copies) {
            self.discard(&copies[0]);
            return None;
        }
        if limited {
            self.copies_entered(id);
        }
        let [child] = copies;
        let child_id = child.cell.id;
        self.enter_networks(core::iter::once(child_id));
        self.admit(|s| file(s, None, child));
        Some(child_id)
    }
}

impl Host {
    /// Makes `snapshot`, taken in another domain than the one `cell`,
    /// restored from it, just entered, one of `cell`'s domain (design D21
    /// of `add-cell-hierarchy`): it keeps referring only to public frames.
    /// `false`, with `cell` taken out of the tree again, if memory cannot
    /// hold the contents of the others.
    #[cold]
    #[inline(never)]
    fn moved(&mut self, snapshot: &mut Snapshot, cell: &Cell) -> bool {
        if snapshot.refers() && !cells_may_take(snapshot.shared_bytes()) {
            self.leave_restored(cell, snapshot.origin());
            return false;
        }
        let mut public = self.public_frames();
        snapshot.move_to(cell.domain, &DirectMap, &mut public);
        true
    }
}

/// Makes the wipe ranges of `live`, a copy that never ran, read as zero
/// (design D22 of `add-cell-hierarchy`); returns whether it could. Kept
/// out of line: copies of cells without wipe ranges never come here.
#[cold]
#[inline(never)]
fn wipe_copy(live: &Live) -> bool {
    let pt = PageTable::<Format>::from_root(live.cell.root);
    let mut frames = cell_frames(&live.frames);
    let ranges = live.demand.wx.wiped();
    sharing(true, || {
        space::wipe(&pt, &mut DirectMap, &mut NewSpace, &mut frames, ranges)
    })
    .is_ok()
}

/// The copy generation of a cell loaded from bytes, which can be loaded
/// any number of times: drawn from the kernel's generator, never `saved`
/// (the generation the bytes hold, [`UNKNOWN_GENERATION`] for a version-10
/// file), so two loads of one file differ but with chance 2^-62. Never 0,
/// and below 2^62, so later copies counting up from it stay far from
/// [`UNKNOWN_GENERATION`].
fn loaded_generation(saved: u64) -> u64 {
    loop {
        let mut word = [0u8; 8];
        RANDOM.with(|g| g.fill(&mut word));
        let generation = (u64::from_le_bytes(word) >> 2) | 1;
        if generation != saved {
            return generation;
        }
    }
}
