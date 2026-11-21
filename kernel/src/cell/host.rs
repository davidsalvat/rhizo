//! The host of cells: spawning them, running them on every CPU until a
//! condition holds, and collecting how they ended.

use super::*;

/// A cell that ended, with its captured output.
#[derive(Debug)]
pub struct Finished {
    pub id: u32,
    pub ending: Ending,
    pub output: Vec<u8>,
    /// The CPUs it ran on (bit `n` for CPU `n`).
    #[cfg_attr(not(feature = "ktest"), allow(dead_code, reason = "in-kernel tests"))]
    pub cpus: u32,
    /// Most CPUs that ran its threads at the same time.
    #[cfg_attr(not(feature = "ktest"), allow(dead_code, reason = "in-kernel tests"))]
    pub peak: u32,
}

/// The cells of the machine: the live ones in the shared scheduler (or on
/// the CPU running them), ended ones kept until the caller looks at them.
/// [`run`](Self::run) uses every CPU. Every other operation acts on one
/// cell with all its threads stopped, between runs or, from `run`'s stop
/// condition, during one: then only the CPUs running that cell stop, and
/// only until it is back in the scheduler (design D1 of
/// `host-ops-per-cell`).
#[derive(Debug)]
pub struct Host {
    pub(super) shared: Global<Shared>,
    pub(super) finished: Vec<Finished>,
    /// Frozen cells and their files: never run, only forked (spec `fork`,
    /// templates).
    pub(super) templates: Vec<(Cell, Fs, WxRanges)>,
    /// When the network card is to be polled next, and what its last poll
    /// said: nanoseconds until it wants polling (design D4 of
    /// `add-virtio-net`; `None`: no card). The boot CPU polls it.
    pub(super) wire_due: u64,
    pub(super) wire_delay: Option<u64>,
    /// The card's address after its last poll.
    pub(super) wire_addr: Option<[u8; 4]>,
    /// Longest the boot CPU waits in [`run`](Self::run) before it looks at
    /// its stop condition again, even with nothing to run (a control
    /// kernel's poll of the serial line, design D4 of `add-control-plane`).
    look_every: Option<u64>,
    /// The output [`output`](Self::output) copied last.
    pub(super) peeked: Vec<u8>,
    /// Whether other CPUs run cells: during a run on several CPUs (see
    /// [`locked`](Self::locked)).
    others: bool,
    /// The time limit of the current run ([`run_until`](Self::run_until)):
    /// the boot CPU never waits past it (design D1 of
    /// `fix-lost-wakeup-smp`).
    until: Option<u64>,
    /// The ownership tree of every cell: live, suspended and templates
    /// (spec `cell-hierarchy`).
    pub(super) tree: rhizo_cap::Tree,
    /// Cells suspended into a snapshot the caller holds, whose node waits
    /// for their resume.
    pub(super) suspended: Vec<u32>,
    /// Cells that ended, their end not yet applied to the tree.
    pub(super) ends: Vec<u32>,
    /// Ends are being applied (not again from inside).
    pub(super) settling: bool,
    /// Templates published, and dropped ones still shared (design D21 of
    /// `add-cell-hierarchy`).
    pub(super) published: Published,
}

/// Words of a running cell's memory ([`Host::watch`]), read through the
/// direct map: their address there, and how many.
#[cfg(any(feature = "ktest", feature = "bench-cell"))]
#[derive(Debug, Clone, Copy)]
pub struct Watched(u64, usize);

#[cfg(any(feature = "ktest", feature = "bench-cell"))]
impl Watched {
    /// Word `i`, as the cell's code changes it on another CPU; only while
    /// the cell keeps the frame ([`Host::watch`]).
    ///
    /// # Panics
    /// If `i` is not one of the words watched.
    pub fn word(&self, i: usize) -> &AtomicU64 {
        assert!(i < self.1, "word {i} is not watched");
        // SAFETY: `Host::watch` found the words aligned, inside one frame of
        // the cell, mapped writable; the direct map maps every frame of RAM
        // read-write, and the cell accesses these words only atomically
        // (`progress`), so reading them as atomics races with nothing.
        unsafe { &*((self.0 + 8 * i as u64) as *const AtomicU64) }
    }
}

impl Host {
    /// An empty host for every CPU online.
    pub fn new() -> Self {
        Self {
            shared: Global::new(Shared {
                sched: Scheduler::new(crate::smp::count()),
                tags: TagPool::new(MAX_TAG.load(Ordering::Relaxed)),
                ended: Vec::new(),
                running: 0,
                idle: 0,
                stopping: false,
                nudged: BTreeSet::new(),
                gangs: Gangs::new(),
                timer: IdleTimer::new(),
                on_cpu: [0; MAX_CPUS],
                claimed: None,
            }),
            finished: Vec::new(),
            templates: Vec::new(),
            wire_due: 0,
            wire_delay: None,
            wire_addr: None,
            look_every: None,
            peeked: Vec::new(),
            others: false,
            until: None,
            tree: rhizo_cap::Tree::new(),
            suspended: Vec::new(),
            ends: Vec::new(),
            settling: false,
            published: Published::default(),
        }
    }

    /// Makes [`run`](Self::run) look at its stop condition at least every
    /// `ns`, and never end by itself (`None`: it ends when no cell can run
    /// again).
    pub fn look_every(&mut self, ns: Option<u64>) {
        self.look_every = ns;
    }

    /// A host whose cells get no address-space tag: every switch flushes the
    /// TLB, as before tags (the `bench-cell` baseline for `cell.switch_ns`).
    #[cfg(feature = "bench-cell")]
    pub(super) fn new_untagged() -> Self {
        let mut host = Self::new();
        host.shared.get_mut().tags = TagPool::new(0);
        host
    }

    /// The shared state, when no other CPU runs: once `run` has joined
    /// them, or as the host goes.
    fn state(&mut self) -> &mut Shared {
        self.shared.get_mut()
    }

    /// Runs `f` on the shared state: under its lock while other CPUs run
    /// cells, without it otherwise (nobody else reaches it then, and host
    /// operations between runs cost what they did before change
    /// `host-ops-per-cell`).
    pub(super) fn locked<R>(&mut self, f: impl FnOnce(&mut Shared) -> R) -> R {
        if self.others {
            self.shared.with(f)
        } else {
            f(self.shared.get_mut())
        }
    }

    /// Takes live cell `id` out of the scheduler for a host operation, with
    /// its wake time if it slept. If CPUs run it, only they are interrupted
    /// and give it back, and the host waits for that (design D2 of
    /// `host-ops-per-cell`); it then comes as runnable. `None` if it is not
    /// live: if it ended, even meanwhile on another CPU, its record is then
    /// in [`finished`](Self::finished).
    pub(super) fn take_cell(&mut self, id: u32) -> Option<(Box<Live>, Option<u64>)> {
        let (running, idle) = self.locked(|s| {
            let found = s.sched.remove(id);
            // This CPU works for the host now, not on the cells it left
            // runnable: an idle CPU takes them.
            let idle = idle_for_runnable(s);
            if let Some(found) = found {
                return (Ok(found), idle);
            }
            let cpus = running_on(s, id);
            if cpus != 0 {
                CLAIM.store(id, Ordering::Relaxed);
            }
            (Err(cpus), idle)
        });
        wake_cpus(idle);
        match running {
            Ok(found) => return Some(found),
            Err(0) => {
                self.collect(); // it may have just ended
                return None;
            }
            Err(cpus) => wake_cpus(cpus),
        }
        loop {
            core::hint::spin_loop();
            let given = self.locked(|s| {
                let live = s.claimed.take();
                // Back, or ended on its CPU.
                let done = live.is_some() || running_on(s, id) == 0;
                if done {
                    CLAIM.store(0, Ordering::Relaxed);
                }
                done.then_some(live)
            });
            match given {
                Some(Some(live)) => return Some((live, None)),
                Some(None) => {
                    self.collect(); // its record, for `finished`
                    return None;
                }
                None => {}
            }
        }
    }

    /// Runs `f` on live cell `id` where it is (in the scheduler, under the
    /// host's lock: `f` is short and takes no cell lock nor [`SHARING`]),
    /// or, if CPUs run it, once they gave it back ([`take_cell`]). `None`
    /// if it is not live.
    ///
    /// [`take_cell`]: Self::take_cell
    pub(super) fn with_live<R>(&mut self, id: u32, f: impl FnOnce(&mut Live) -> R) -> Option<R> {
        let mut f = Some(f);
        let found = self.locked(|s| s.sched.find_mut(id).map(|l| f.take().map(|f| f(l))));
        if let Some(r) = found {
            return r;
        }
        let (mut live, wake) = self.take_cell(id)?;
        let r = f.take().map(|f| f(&mut live));
        self.requeue(live, wake);
        r
    }

    /// Runs `f` on the shared state to put cells in it, then wakes an idle
    /// CPU to take the runnable ones (during a run; none idles between
    /// runs).
    pub(super) fn admit<R>(&mut self, f: impl FnOnce(&mut Shared) -> R) -> R {
        if self.others {
            return self.admit_running(f);
        }
        f(self.shared.get_mut())
    }

    /// [`admit`](Self::admit) while other CPUs run cells. Out of line:
    /// inlined into the spawn paths, it made an AArch64 spawn about 530
    /// instructions slower (`cell.spawn_insns`) through the code generated
    /// around it, not its own.
    #[inline(never)]
    fn admit_running<R>(&mut self, f: impl FnOnce(&mut Shared) -> R) -> R {
        let (r, wake) = self.shared.with(|s| (f(s), idle_for_runnable(s)));
        wake_cpus(wake);
        r
    }

    /// Moves the cells that ended into [`finished`](Self::finished) (other
    /// CPUs may be adding to them).
    fn collect(&mut self) {
        let ended = self.shared.with(|s| core::mem::take(&mut s.ended));
        if ended.is_empty() {
            return;
        }
        self.ends.extend(ended.iter().map(|f| f.id));
        self.finished.extend(ended);
        self.settle();
    }

    /// A new live cell's record: its tag, nothing flushed yet.
    pub(super) fn new_live(
        &mut self,
        cell: Cell,
        output: bool,
        echo: bool,
        mut fs: Fs,
    ) -> Box<Live> {
        // Changed only when they differ: setting them copies shared tables.
        let limits = (cell.quota.files() as usize, cell.quota.sockets as usize);
        if fs.limits() != limits {
            fs.set_limits(limits.0, limits.1);
        }
        let tag = self.locked(|s| s.tags.alloc());
        if tag.is_none() {
            prof::add(Count::Untagged, 1);
        }
        let frames = Meter::new(0, cell.quota.frames());
        Box::new(Live {
            cell,
            tag: tag.unwrap_or(0),
            clean: 0,
            cpus: 0,
            shares: false,
            abi: Abi::new(user::ABI),
            output: output.then(Vec::new),
            echo,
            random: None,
            demand: OnDemand::default(),
            frames,
            cpu_ticks: 0,
            fs,
            deadlines: Vec::new(),
            io_woken: false,
            host_wake: false,
            ready: Ready::default(),
            last: 0,
            peak: 1,
            hold: false,
        })
    }

    /// Creates a cell from the static executable `image` with `argv` and
    /// makes it runnable. Its output goes to the console when `echo` is set
    /// and is kept in [`Finished::output`] when `capture` is set. The image
    /// is loaded once and kept: later spawns of it copy that space (design
    /// D1 of `add-image-cache`).
    pub fn spawn(
        &mut self,
        image: &'static [u8],
        argv: &[&[u8]],
        capture: bool,
        echo: bool,
    ) -> Result<u32, ElfError> {
        self.spawn_with_files(image, argv, no_files(), capture, echo)
    }

    /// [`spawn`](Self::spawn) with `files` as the cell's file system (spec
    /// `files`, initial files).
    pub fn spawn_with_files(
        &mut self,
        image: &'static [u8],
        argv: &[&[u8]],
        files: Fs,
        capture: bool,
        echo: bool,
    ) -> Result<u32, ElfError> {
        self.spawn_with_env(image, argv, &[], files, capture, echo)
    }

    /// [`spawn_with_files`](Self::spawn_with_files) with `envp` as the
    /// cell's environment.
    pub fn spawn_with_env(
        &mut self,
        image: &'static [u8],
        argv: &[&[u8]],
        envp: &[&[u8]],
        files: Fs,
        capture: bool,
        echo: bool,
    ) -> Result<u32, ElfError> {
        let _p = cost::scope(Part::Spawn);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let random = start_random();
        let kernel = user::cell_kernel_half();
        let kernel_half = user::CELL_ROOT_HAS_KERNEL_HALF.then_some(&kernel);
        let parsed = Image::parse(image, user::MACHINE)?;
        let used = Meter::new(0, u64::MAX);
        let cell = rhizo_cell::start::<Format>(
            id,
            &parsed,
            user::MACHINE,
            image_space(image, &parsed)?,
            kernel_half,
            &mut DirectMap,
            &mut cell_frames(&used),
            argv,
            envp,
            random,
        )?;
        let mut live = self.new_live(cell, capture, echo, files);
        live.frames.set_used(used.used());
        if !self.enter_root(&mut live.cell) {
            self.discard(&live);
            return Err(ElfError::OutOfMemory);
        }
        self.admit(|s| s.sched.place(live));
        Ok(id)
    }

    /// Creates a cell running the executable at `path` in `files`, its
    /// file system, with `argv` and `envp` (spec `cells`, dynamically
    /// linked executables): the executable and its interpreter are mapped
    /// from the files' pages, which every cell mapping the same bytes shares
    /// (design D1 of `add-dynamic-linking`). Output as for
    /// [`spawn`](Self::spawn).
    #[cfg_attr(
        not(any(feature = "ktest", feature = "bench-cell")),
        allow(dead_code, reason = "used by in-kernel tests and benchmarks")
    )]
    pub fn spawn_file(
        &mut self,
        files: Fs,
        path: &[u8],
        argv: &[&[u8]],
        envp: &[&[u8]],
        capture: bool,
        echo: bool,
    ) -> Result<u32, ExecError> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let random = start_random();
        let kernel = user::cell_kernel_half();
        let kernel_half = user::CELL_ROOT_HAS_KERNEL_HALF.then_some(&kernel);
        let cell = rhizo_abi::exec::<Format, _, _>(
            id,
            user::MACHINE,
            &files,
            path,
            kernel_half,
            &mut DirectMap,
            &mut NewSpace,
            &mut KernelFrames,
            &mut KernelPages,
            argv,
            envp,
            random,
        )?;
        let mut live = self.new_live(cell, capture, echo, files);
        if !self.enter_root(&mut live.cell) {
            self.discard(&live);
            return Err(ElfError::OutOfMemory.into());
        }
        self.admit(|s| s.sched.place(live));
        Ok(id)
    }

    /// Runs one scheduling step on this CPU alone: the cell that has waited
    /// longest for one slice (until it ends, yields, sleeps or is
    /// preempted), or, if every cell sleeps, a wait for the first to wake.
    /// Returns `false` when no cell is left.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests")
    )]
    pub fn step(&mut self) -> bool {
        self.poll_wire(now_ns());
        match take(&self.shared, 0, true) {
            Take::Run(live, now) => run_slice(&self.shared, 0, live, now, false),
            Take::Join(gang) => join_gang(&self.shared, 0, gang),
            Take::Wait(wake) => {
                self.wait_or_poll(wake, false);
            }
            Take::Done => {
                self.wait_or_poll(None, true);
            }
            Take::Stop | Take::Look => {}
        }
        self.collect();
        self.live() > 0
    }

    /// The `words` 8-byte words of live cell `id`'s memory at `addr`, to read
    /// while the cell runs (in-kernel tests and benchmarks that watch a cell
    /// without stopping it, change `host-ops-per-cell`). `None` unless they
    /// are aligned, in one page, and mapped writable (the cell's own frame,
    /// which it keeps until it ends, is forked or is suspended).
    #[cfg(any(feature = "ktest", feature = "bench-cell"))]
    pub fn watch(&mut self, id: u32, addr: u64, words: usize) -> Option<Watched> {
        let fits = addr.is_multiple_of(8) && addr % FRAME_SIZE + 8 * words as u64 <= FRAME_SIZE;
        let at = self
            .with_live(id, |live| {
                PageTable::<Format>::from_root(live.cell.root).translate(&DirectMap, addr)
            })
            .flatten()?;
        (fits && at.flags.is_writable()).then(|| Watched(DirectMap::virt(at.phys), words))
    }

    /// [`run`](Self::run) until monotonic time `at` at the latest: the boot
    /// CPU's waits end by then, whatever the cells wait for and whichever
    /// CPU's timer holds their deadlines (design D1 of
    /// `fix-lost-wakeup-smp`).
    #[cfg_attr(
        not(any(feature = "ktest", feature = "bench-cell")),
        allow(dead_code, reason = "used by in-kernel tests and benchmarks")
    )]
    ///
    /// `stop` may operate on cells, as with `run`, and may bring the boot
    /// CPU's next wait forward with [`ask_by`](Self::ask_by): a host
    /// operation due before `at` then runs on time on several CPUs too.
    pub fn run_until(&mut self, at: u64, mut stop: impl FnMut(&mut Self) -> bool) {
        self.run(|h| {
            h.until = Some(at); // `stop` may bring it forward
            now_ns() >= at || stop(h)
        });
        self.until = None;
    }

    /// From the stop condition of [`run_until`](Self::run_until): the boot
    /// CPU asks it again by monotonic time `at` at the latest, even with
    /// nothing to run (change `host-ops-per-cell`, its server workload's
    /// operations). Holds until the condition is next asked.
    #[cfg_attr(
        not(any(feature = "ktest", feature = "bench-cell")),
        allow(dead_code, reason = "used by benchmarks")
    )]
    pub fn ask_by(&mut self, at: u64) {
        self.until = Some(self.until.map_or(at, |until| until.min(at)));
    }

    /// Runs cells on every CPU until none is left that can ever run or
    /// `stop` holds (checked on this CPU whenever it gives up a cell or a
    /// cell ends), then waits for the other CPUs to give back theirs.
    /// `stop` may operate on cells (suspend, resume, fork, kill...): the
    /// other CPUs go on running theirs meanwhile (design D1 of
    /// `host-ops-per-cell`).
    pub fn run(&mut self, mut stop: impl FnMut(&mut Self) -> bool) {
        if stop(self) {
            return;
        }
        let cpus = crate::smp::count();
        if cpus > 1 {
            let shared = &self.shared as *const Global<Shared> as u64;
            for cpu in 1..cpus {
                crate::smp::post(cpu, run_cpu, shared);
            }
            self.others = true;
        }
        loop {
            self.poll_wire(now_ns());
            match take(&self.shared, 0, false) {
                Take::Run(live, now) => run_slice(&self.shared, 0, live, now, cpus > 1),
                Take::Join(gang) => join_gang(&self.shared, 0, gang),
                Take::Wait(wake) => {
                    self.wait_or_poll(wake, false);
                }
                Take::Look => {}
                Take::Done => {
                    if !self.wait_or_poll(None, true) {
                        break;
                    }
                }
                Take::Stop => break,
            }
            self.collect();
            if stop(self) {
                break;
            }
        }
        if cpus > 1 {
            self.shared.with(|s| s.stopping = true);
            for cpu in 1..cpus {
                crate::smp::wake(cpu);
            }
            for cpu in 1..cpus {
                crate::smp::join(cpu);
            }
            self.others = false;
            crate::heap::reclaim_tables(); // the other CPUs can flush now
            let state = self.state();
            state.stopping = false;
            state.idle = 0;
            state.timer = IdleTimer::new();
            debug_assert!(state.gangs.is_empty(), "every CPU left every cell");
            debug_assert!(state.claimed.is_none(), "the host took what it claimed");
        }
        // Between runs every live cell is in the scheduler: a cell nudged
        // after it ended is forgotten.
        let state = self.state();
        if !state.nudged.is_empty() {
            let sched = &state.sched;
            state.nudged.retain(|&id| sched.find(id).is_some());
        }
        self.collect();
    }

    /// The boot CPU has nothing to run: cells waiting on descriptors may wait
    /// for the network card, so it polls the card now (a cell may just have
    /// opened a connection) and, if that changed nothing, waits until `wake`
    /// or until the card wants polling (design D4 of `add-virtio-net`).
    /// Without `wake` it waits for a wake-up, unless `done` (no deadline
    /// anywhere) and no CPU runs a cell; in a run with a time limit, never
    /// past it. Returns `false` when nothing can ever wake a cell.
    fn wait_or_poll(&mut self, wake: Option<u64>, done: bool) -> bool {
        let now = now_ns();
        let waiting = self.shared.with(|s| s.sched.waiting()) > 0;
        let mut wake = match (wake, self.look_every) {
            (Some(w), Some(every)) => Some(w.min(now.saturating_add(every))),
            (w, every) => w.or(every.map(|e| now.saturating_add(e))),
        };
        if waiting && self.wire_delay.is_some() {
            self.wire_due = 0;
            self.poll_wire(now);
            if WOKEN.load(Ordering::Acquire) {
                return true; // the next take wakes them
            }
            if let Some(delay) = self.wire_delay.filter(|&d| d != u64::MAX) {
                let soon = now.saturating_add(delay.max(1));
                wake = Some(wake.map_or(soon, |w| w.min(soon)));
            }
        }
        // Never past the run's time limit: another CPU's timer may hold the
        // cells' deadline, and nothing else may wake this CPU.
        match wake {
            Some(at) => {
                wait(Some(self.until.map_or(at, |until| at.min(until))));
                true
            }
            None if !done || self.shared.with(|s| s.running) > 0 => {
                wait(self.until);
                true
            }
            None => false,
        }
    }

    /// Ends `live`, taken out of the scheduler, and keeps what remains of
    /// it.
    pub(super) fn end(&mut self, live: Box<Live>, ending: Ending) {
        self.locked(|s| s.nudged.remove(&live.cell.id));
        let (finished, tag) = end(live, ending);
        if tag != 0 {
            self.locked(|s| s.tags.free(tag));
        }
        self.ended(finished.id, ending);
        self.finished.push(finished);
        self.settle();
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.settling = true; // every cell ends: nothing to cascade
        while let Some(live) = self.state().sched.pop() {
            self.end(live, Ending::Killed);
        }
        while let Some((template, ..)) = self.templates.first() {
            let id = template.id;
            self.drop_template(id);
        }
        self.drop_held();
    }
}
