//! What the host asks of one cell between runs: its output and input,
//! files, quota, wake-ups, and killing it.

use super::*;

impl Host {
    /// Page-table frames of the own half of live cell `id`.
    #[cfg_attr(
        not(feature = "bench-cell"),
        allow(dead_code, reason = "benchmark metric")
    )]
    pub fn table_frames(&mut self, id: u32) -> Option<usize> {
        self.with_live(id, |live| {
            PageTable::<Format>::from_root(live.cell.root).lower_half_tables(&DirectMap)
        })
    }

    /// What live cell `id` has printed so far (when captured).
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn output(&mut self, id: u32) -> Option<&[u8]> {
        // Copied out: other CPUs may run the cell once the lock is gone.
        let mut copy = core::mem::take(&mut self.peeked);
        let found = self.with_live(id, |live| {
            live.output.as_ref().map(|o| {
                copy.clear();
                copy.extend_from_slice(o);
            })
        });
        self.peeked = copy;
        found.flatten().map(|()| self.peeked.as_slice())
    }

    /// Takes what live cell `id` has printed since the last call (when
    /// captured): its capture starts empty again, so a cell that keeps
    /// printing is bounded per call, not for its whole life.
    #[cfg_attr(
        not(feature = "control"),
        allow(dead_code, reason = "used by the control kernel")
    )]
    pub fn take_output(&mut self, id: u32) -> Option<Vec<u8>> {
        self.with_live(id, |live| live.output.as_mut().map(core::mem::take))
            .flatten()
    }

    /// Pages of live cell `id` still waiting in its snapshot (lazy resume).
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn pending_pages(&mut self, id: u32) -> Option<usize> {
        self.with_live(id, |live| live.demand.lazy.as_ref().map_or(0, |l| l.left))
    }

    /// Whether live cell `id` sleeps: none of its threads can run, and one
    /// waits for a deadline, not for a descriptor (a host's safe point to
    /// suspend or freeze it, design D3 of `add-agent-workload`).
    #[cfg_attr(
        not(any(feature = "ktest", feature = "bench-cell")),
        allow(dead_code, reason = "used by in-kernel tests and benchmarks")
    )]
    pub fn asleep(&mut self, id: u32) -> bool {
        // A cell a CPU runs is not asleep.
        self.locked(|s| {
            s.sched.find(id).is_some_and(|l| {
                let threads = &l.cell.threads;
                !threads.is_runnable() && !threads.waits_on_io() && threads.wake_at().is_some()
            })
        })
    }

    /// Whether live cell `id` waits: none of its threads can run.
    #[cfg_attr(
        not(any(feature = "ktest", feature = "bench-cell")),
        allow(dead_code, reason = "used by in-kernel tests and benchmarks")
    )]
    pub fn waits(&mut self, id: u32) -> bool {
        // A cell a CPU runs does not wait.
        self.locked(|s| {
            s.sched
                .find(id)
                .is_some_and(|l| !l.cell.threads.is_runnable())
        })
    }

    /// Creates or replaces file `path` with `data` in live cell `id`'s
    /// files; returns whether it could (how a host hands a forked cell its
    /// part, design D3 of `add-agent-workload`).
    #[cfg_attr(
        not(any(feature = "ktest", feature = "bench-cell")),
        allow(dead_code, reason = "used by in-kernel tests and benchmarks")
    )]
    pub fn write_file(&mut self, id: u32, path: &[u8], data: &[u8]) -> bool {
        self.with_live(id, |l| l.fs.put_file(path, data).is_ok())
            .unwrap_or(false)
    }

    /// Writes `data` into live cell `id`'s standard input from the host, or
    /// closes the host's end of it when `data` is `None` (spec
    /// `control-plane`, design D3 of `add-interactive-chat`), then wakes
    /// its threads waiting on descriptors: they make their calls again and
    /// find the bytes, or the end of file. `None` if the cell is not live.
    #[cfg_attr(
        not(feature = "control"),
        allow(dead_code, reason = "used by the control kernel")
    )]
    pub fn input(&mut self, id: u32, data: Option<&[u8]>) -> Option<rhizo_fs::Result<()>> {
        let (mut live, _) = self.take_cell(id)?;
        let result = match data {
            // The bytes wait in the kernel heap, as a cell's own pipe writes.
            Some(data) if !cells_may_take(data.len() as u64) => Err(rhizo_fs::Error::NoSpace),
            Some(data) => live.fs.push_input(data),
            None => live.fs.close_input(),
        };
        // Filed back from outside the scheduler, its threads waiting on
        // descriptors make their calls again.
        self.admit(|s| file(s, None, live));
        Some(result)
    }

    /// Whether live cell `id` has printed something not taken yet and
    /// waits (none of its threads can run): the moment to send its output
    /// on (design D3 of `add-interactive-chat`).
    #[cfg_attr(
        not(feature = "control"),
        allow(dead_code, reason = "used by the control kernel")
    )]
    pub fn has_output(&self, id: u32) -> bool {
        self.shared.with(|s| {
            s.sched.find(id).is_some_and(|l| {
                !l.cell.threads.is_runnable() && l.output.as_ref().is_some_and(|o| !o.is_empty())
            })
        })
    }

    /// Ends the sleeps of live cell `id`'s threads and times out their timed
    /// waits, as an event from outside would (the benchmark host's stand-in
    /// for a user's message, design D1 of `add-reference-workloads`);
    /// returns whether the cell exists. A cell none of whose threads waits
    /// with a deadline yet (it is still on its way to its sleep) keeps the
    /// wake-up for its next one (design D5 of `add-socket-wait-queues`).
    #[cfg_attr(
        not(feature = "bench-cell"),
        allow(dead_code, reason = "used by the reference workloads")
    )]
    pub fn wake(&mut self, id: u32) -> bool {
        let Some((mut live, _)) = self.take_cell(id) else {
            return false;
        };
        live.host_wake = live.cell.threads.wake_at().is_none();
        live.expire_timers();
        self.admit(|s| file(s, None, live));
        true
    }

    /// Sets the quotas of live cell `id` (spec `isolation`); returns whether
    /// it exists. Memory, descriptors and sockets it holds already stay; it
    /// gets no more past the new limits.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn set_quota(&mut self, id: u32, quota: Quota) -> bool {
        let live = self
            .with_live(id, |live| live.cell.quota.files = quota.files)
            .is_some();
        live && self.set_limits_from(id, &quota, None)
    }

    /// [`set_quota`](Self::set_quota), and at most `cells` descendants
    /// (spec `cell-hierarchy`, budgets).
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn set_limits(&mut self, id: u32, quota: Quota, cells: u32) -> bool {
        self.set_quota(id, quota) && self.set_limits_from(id, &quota, Some(cells))
    }

    /// The frame live cell `id` maps at `addr`, if any: which frames cells
    /// share (design D21 of `add-cell-hierarchy`).
    #[cfg(feature = "ktest")]
    pub fn frame_of(&mut self, id: u32, addr: u64) -> Option<u64> {
        self.with_live(id, |live| {
            let pt = PageTable::<Format>::from_root(live.cell.root);
            pt.translate(&DirectMap, addr)
                .map(|t| t.phys & !(FRAME_SIZE - 1))
        })?
    }

    /// Frames counted against the memory quota of live cell `id`.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn charged_frames(&mut self, id: u32) -> Option<u64> {
        self.with_live(id, |live| live.frames.used())
    }

    /// Ends live cell `id` if it exists; returns whether it did.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn kill(&mut self, id: u32) -> bool {
        match self.take_cell(id) {
            Some((live, _)) => {
                self.end(live, Ending::Killed);
                true
            }
            None => false,
        }
    }

    /// How cell `id` ended, if it has.
    #[cfg_attr(
        not(any(feature = "ktest", feature = "bench-cell")),
        allow(dead_code, reason = "used by in-kernel tests and benchmarks")
    )]
    pub fn finished(&self, id: u32) -> Option<&Finished> {
        self.finished.iter().find(|f| f.id == id)
    }

    /// Whether a cell ended (or was suspended) since the records were last
    /// taken.
    #[cfg_attr(
        not(feature = "control"),
        allow(dead_code, reason = "used by the control kernel")
    )]
    pub fn has_finished(&self) -> bool {
        !self.finished.is_empty()
    }

    /// Takes the record of every cell that ended so far.
    #[cfg_attr(
        not(feature = "bench-cell"),
        allow(dead_code, reason = "used by the reference workloads")
    )]
    pub fn take_finished(&mut self) -> Vec<Finished> {
        core::mem::take(&mut self.finished)
    }

    /// Number of live cells waiting on descriptors.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn waiting(&self) -> usize {
        self.shared.with(|s| s.sched.waiting())
    }

    /// Number of live cells.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn live(&self) -> usize {
        self.shared.with(|s| s.sched.len() + s.running)
    }
}
