//! Suspending, resuming and forking cells, and templates (specs
//! `snapshots` and `fork`).

use super::*;

/// How [`Host::resume`] restores memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    /// Every page before the cell runs.
    Eager,
    /// Each page the first time it is touched.
    Lazy,
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
        if !cells_may_take(bytes) {
            self.requeue(live, wake);
            return None;
        }
        let pt = PageTable::<Format>::from_root(live.cell.root);
        if let Some(lazy) = live.demand.lazy.as_mut() {
            lazy.finish(&pt, &live.frames);
        }
        sync_fp(&mut live, true);
        let (files, sockets) = export_sockets(&live.fs);
        // Refers to the pages it shares instead of copying them (design D1
        // of `add-shared-snapshots`); its tables become its own first, under
        // the lock of shared tables when it has some.
        let snapshot = sharing(live.shares, || {
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
        self.end(live, Ending::Suspended);
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
        let mut live = self.restore(snapshot, mode, capture, echo)?;
        live.cell.threads.wake_io();
        let id = live.cell.id;
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
                .sum();
            if !cells_may_take(bytes) {
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
        let cell = snapshot.cell(id, pt.root(), now);
        let deadlines = snapshot.deadlines(now);
        let mut fs = files_of(&snapshot);
        import_sockets(&mut fs, &snapshot.sockets());
        let wx = snapshot.wx();
        let mut lazy = Lazy::new(snapshot);
        let used = Meter::new(1, u64::MAX); // the root
        if mode == Resume::Eager {
            lazy.finish(&pt, &used);
        }
        let mut live = self.new_live(cell, capture, echo, fs);
        live.frames.set_used(used.used());
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
        let pt = PageTable::<Format>::from_root(parent.cell.root);
        if let Some(lazy) = parent.demand.lazy.as_mut() {
            lazy.finish(&pt, &parent.frames);
        }
        parent.demand.lazy = None;
        sync_fp(&mut parent, false);
        let mut children = Vec::with_capacity(n);
        for _ in 0..n {
            let Some(child) = self.child_of(
                &parent.cell,
                &parent.fs,
                &parent.demand.wx,
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
        if children.len() < n {
            // All or nothing: the children made so far never ran.
            for child in &children {
                self.discard(child);
            }
            return None;
        }
        let ids = children.iter().map(|c| c.cell.id).collect();
        for child in children {
            self.requeue(child, wake);
        }
        Some(ids)
    }

    /// Frees the memory, tag and sockets of a cell that never ran, leaving
    /// no trace (no [`Finished`]).
    fn discard(&mut self, live: &Live) {
        if live.tag != 0 {
            self.locked(|s| s.tags.free(live.tag));
        }
        release_sockets(&live.fs, live.cell.id);
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
    /// `host-ops-per-cell`).
    fn child_of(
        &mut self,
        cell: &Cell,
        fs: &Fs,
        wx: &WxRanges,
        capture: bool,
        echo: bool,
    ) -> Option<Box<Live>> {
        let pt = PageTable::<Format>::from_root(cell.root);
        let child = pt.fork_lower_half(&mut DirectMap, &mut KernelFrames).ok()?;
        // The child has the cell's quotas and none of its usage (design D3
        // of `add-cell-quotas`): what it shares stays counted to the cell.
        let cell = Cell {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            root: child.root(),
            cpu_ns: 0,
            ..cell.clone()
        };
        let mut live = self.new_live(cell, capture, echo, fork_files(fs));
        live.frames.set_used(1); // its root
        live.shares = true;
        live.demand.wx = wx.clone();
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
        let Some((mut live, _)) = self.take_cell(id) else {
            return false;
        };
        let pt = PageTable::<Format>::from_root(live.cell.root);
        if let Some(lazy) = live.demand.lazy.as_mut() {
            lazy.finish(&pt, &live.frames);
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
        let snapshot = sharing(true, || {
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
        close_parked(&snapshot);
        Some(snapshot)
    }

    /// Drops template `id`: its memory and sockets go back once no copy
    /// shares them. Returns whether it existed.
    pub fn drop_template(&mut self, id: u32) -> bool {
        let Some(index) = self.templates.iter().position(|(c, ..)| c.id == id) else {
            return false;
        };
        let (template, fs, _) = self.templates.swap_remove(index);
        space::destroy(
            PageTable::<Format>::from_root(template.root),
            &mut DirectMap,
            &mut KernelFrames,
        );
        release_sockets(&fs, template.id);
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
        let (cell, fs, wx) = &self.templates[index];
        let (cell, fs, wx) = (cell.clone(), fs.clone(), wx.clone());
        let mut child = self.child_of(&cell, &fs, &wx, capture, echo)?;
        child.expire_timers();
        let child_id = child.cell.id;
        self.admit(|s| file(s, None, child));
        Some(child_id)
    }
}
