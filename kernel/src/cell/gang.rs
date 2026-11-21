//! A cell whose threads run on several CPUs at once (a [`Gang`]): the
//! cell lock, joining and leaving, and TLB shootdowns.

use super::*;

/// A cell whose threads run on several CPUs at once (design D3 of
/// `add-parallel-threads`): the cell behind its own lock, and what the CPUs
/// running it read without that lock.
pub(super) struct Gang {
    /// The cell, its threads and where they run: locked around system
    /// calls, faults that change its tables, and choosing threads.
    cell: Global<GangCell>,
    id: u32,
    root: u64,
    tag: u16,
    /// [`Live::clean`], read as CPUs enter the cell's code.
    clean: AtomicU32,
    /// The cell ended: the CPUs running its threads leave (design D6).
    ending: AtomicBool,
    /// Its translations are shrinking: the other CPUs keep out of its code
    /// (design D7; x86_64 only).
    hold: AtomicBool,
}

/// What a gang's lock protects.
struct GangCell {
    live: Box<Live>,
    /// The thread each CPU runs (0: none).
    busy: [u32; MAX_CPUS],
    /// How the cell ended, once it has.
    ending: Option<Ending>,
    /// What it offers to other CPUs in [`Shared::gangs`]: runnable threads
    /// no CPU runs, and the earliest deadline of its waiting threads.
    offer: (u32, Option<u64>),
}

/// A running [`Gang`], as `Shared::gangs` records it, and its cell's
/// identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GangPtr(pub(super) NonNull<Gang>, pub(super) u32);

// SAFETY: the pointer is a handle that CPUs pass under the host's lock; the
// gang behind it is shared through its own lock and atomics only.
unsafe impl Send for GangPtr {}

/// The cell whose code each CPU runs and a sequence number, odd while it
/// runs it (`id << 32 | sequence`; design D7, x86_64 only).
static IN_CELL: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// What a CPU does after a section under a gang's lock.
enum After {
    /// Its thread runs on.
    Resume,
    /// Its thread stopped and was put back: this one (whose registers the
    /// CPU now holds) runs next, or the CPU leaves.
    Switch(Option<u32>),
    /// It leaves the cell (its thread was put back, or the cell ended).
    Leave,
}

impl Gang {
    /// Runs `f` under the cell's lock, then offers what the cell has for
    /// other CPUs and lets them back into its code.
    fn locked<R>(
        &self,
        shared: &Global<Shared>,
        ptr: GangPtr,
        f: impl FnOnce(&mut GangCell) -> R,
    ) -> R {
        self.cell.with(|cell| {
            let result = f(cell);
            cell.offer(shared, ptr);
            if self.hold.load(Ordering::Relaxed) {
                self.hold.store(false, Ordering::Release);
            }
            result
        })
    }

    /// Keeps every other CPU out of the cell's code until the end of the
    /// section under the cell's lock (design D7): marks them stale,
    /// interrupts those that run its code, and waits until they left it.
    /// Nothing to do where TLB invalidations are broadcast.
    pub(super) fn hold_others(&self, cpu: usize) {
        if user::TLB_FLUSH_IS_BROADCAST || self.hold.swap(true, Ordering::SeqCst) {
            return;
        }
        self.clean
            .store(flushed_by_kernel_on(cpu), Ordering::SeqCst);
        let mut seen = [0u64; MAX_CPUS];
        for (other, word) in seen.iter_mut().enumerate().take(crate::smp::count()) {
            let now = IN_CELL[other].load(Ordering::SeqCst);
            if other != cpu && now >> 32 == self.id as u64 && now & 1 == 1 {
                *word = now;
                crate::smp::wake(other);
            }
        }
        for (other, &word) in seen.iter().enumerate() {
            while word != 0 && IN_CELL[other].load(Ordering::Acquire) == word {
                core::hint::spin_loop();
            }
        }
    }

    /// Runs thread code of the cell on `cpu` from `regs`; `None` when the
    /// cell ended. Where flushes are per CPU, publishes that this CPU runs
    /// the cell's code, and stays out while another CPU holds it out
    /// (design D7: either this CPU sees the hold, or the holder sees it here
    /// and interrupts it).
    fn enter(&self, cpu: usize, regs: &mut Registers) -> Option<Trap> {
        // This CPU's word while it runs the cell's code, and after.
        let (mut inside, mut outside) = (0, 0);
        if !user::TLB_FLUSH_IS_BROADCAST {
            loop {
                let sequence = (IN_CELL[cpu].load(Ordering::Relaxed) as u32).wrapping_add(1) | 1;
                inside = (self.id as u64) << 32 | sequence as u64;
                outside = (self.id as u64) << 32 | sequence.wrapping_add(1) as u64;
                IN_CELL[cpu].store(inside, Ordering::SeqCst);
                if !self.hold.load(Ordering::SeqCst) {
                    break;
                }
                IN_CELL[cpu].store(outside, Ordering::SeqCst);
                self.cell.with(|_| ()); // until the holder's section ends
            }
        }
        let trap = if self.ending.load(Ordering::Acquire) {
            None
        } else {
            let flush = self.clean.load(Ordering::SeqCst) & (1 << cpu) == 0;
            if flush {
                self.clean.fetch_or(flushed_on(cpu), Ordering::SeqCst);
            }
            let _p = cost::scope(Part::Cell);
            Some(user::enter_user(self.root, self.tag, flush, regs))
        };
        if inside != 0 {
            IN_CELL[cpu].store(outside, Ordering::Release);
        }
        trap
    }
}

impl GangCell {
    /// Records how many CPUs run the cell's threads now, if that is the
    /// most so far: each holds its thread from the moment it is chosen until
    /// it is put back, so those stints overlap.
    fn count_busy(&mut self) {
        let busy = self.busy.iter().filter(|&&tid| tid != 0).count() as u32;
        self.live.peak = self.live.peak.max(busy);
    }

    /// Chooses the next thread for `cpu` (turns by identifier, skipping the
    /// threads other CPUs run; design D5) and copies its registers into
    /// `regs`, the CPU's; `None` if there is none.
    fn choose(&mut self, cpu: usize, now: u64, regs: &mut Registers) -> Option<u32> {
        if self.ending.is_some() {
            return None;
        }
        let busy = self.busy;
        let threads = &mut self.live.cell.threads;
        let tid = threads.pick(now, self.live.last, |t| busy.contains(&t))?;
        *regs = threads.get(tid)?.regs;
        self.busy[cpu] = tid;
        self.live.last = tid;
        self.count_busy();
        self.live.cpus |= 1 << cpu;
        Some(tid)
    }

    /// Puts back thread `tid`, which `cpu` stops running: its registers
    /// `regs` (unless the table holds them already), and its FP/SIMD state
    /// if this CPU's registers hold it, before another CPU can choose it.
    fn put_back(&mut self, cpu: usize, tid: u32, regs: Option<&Registers>) {
        self.busy[cpu] = 0;
        let key = fp_key(self.live.cell.id, tid);
        let thread = self.live.cell.threads.get_mut(tid);
        if let Some(thread) = thread {
            if let Some(regs) = regs {
                thread.regs = *regs;
            }
            if fp_owner() == key {
                user::fp_save(&mut thread.regs.fp);
            }
        }
        if fp_owner() == key {
            set_fp_owner(FP_STALE);
        }
    }

    /// Ends the cell: records how, and interrupts the other CPUs running its
    /// threads so they leave (design D6).
    fn end(&mut self, gang: &Gang, cpu: usize, ending: Ending) {
        self.ending.get_or_insert(ending);
        gang.ending.store(true, Ordering::Release);
        self.busy[cpu] = 0;
        for (other, &tid) in self.busy.iter().enumerate() {
            if tid != 0 {
                crate::smp::wake(other);
            }
        }
    }

    /// The system call of thread `tid`, whose registers are `regs`, running
    /// on `cpu` (design D4: it becomes the current thread for the call).
    fn syscall(&mut self, gang: &Gang, cpu: usize, tid: u32, regs: &mut Registers) -> After {
        if self.ending.is_some() {
            return After::Leave; // nothing takes effect after the end
        }
        let threads = &mut self.live.cell.threads;
        threads.make_current(tid);
        threads.current_mut().regs = *regs;
        let pt = PageTable::<Format>::from_root(gang.root);
        let fp_in_cpu = fp_owner() == fp_key(gang.id, tid);
        let (outcome, _) = syscall(&mut self.live, pt, fp_in_cpu, Some((gang, cpu)));
        match outcome {
            Outcome::Resume => {
                *regs = *self.live.cell.regs();
                After::Resume
            }
            Outcome::Yield => {
                self.put_back(cpu, tid, None);
                After::Leave
            }
            Outcome::Block => {
                self.put_back(cpu, tid, None);
                After::Switch(self.choose(cpu, now_ns(), regs))
            }
            Outcome::Exit(status) => {
                self.end(gang, cpu, Ending::Exit(status));
                After::Leave
            }
        }
    }

    /// A fault of thread code on `cpu`: resolved (a page brought in, the
    /// stack grown, a copy-on-write page copied, or a page another CPU
    /// mapped meanwhile), or the cell ends. Returns whether the thread runs
    /// on.
    fn fault(&mut self, gang: &Gang, cpu: usize, fault: &UserFault, pc: u64) -> bool {
        if self.ending.is_some() {
            return false;
        }
        let live = &mut *self.live;
        let pt = PageTable::<Format>::from_root(gang.root);
        if let Some(addr) = fault.missing_page() {
            if live.demand.lazy.as_mut().is_some_and(|l| {
                let _p = cost::scope(Part::Fault);
                l.page_in(&pt, &mut cell_frames(&live.frames), addr)
            }) {
                if live.demand.lazy.as_ref().is_some_and(|l| l.left == 0) {
                    live.demand.lazy = None;
                }
                return true;
            }
            if grow_stack(&pt, &mut cell_frames(&live.frames), addr) || back_wx(live, &pt, addr) {
                return true;
            }
            if pt.translate(&DirectMap, addr).is_some() {
                CpuTlb.flush_page(addr & !(FRAME_SIZE - 1));
                return true;
            }
        }
        if let Some(addr) = fault.write_denied()
            && sharing(live.shares, || {
                resolve_write(&pt, &mut CellTlb::new(Some((gang, cpu))), addr)
            })
        {
            return true;
        }
        // Write-or-execute pages (design D3 of `node-jit`).
        if let Some(addr) = fault.exec_denied()
            && sharing(live.shares, || {
                resolve_exec(&pt, &mut CellTlb::new(Some((gang, cpu))), addr)
            })
        {
            return true;
        }
        if let Some(addr) = fault.write_denied()
            && !writes_own_code(&pt, pc, addr)
            && sharing(live.shares, || {
                resolve_write_wx(&pt, &mut CellTlb::new(Some((gang, cpu))), addr)
            })
        {
            return true;
        }
        let mut services = KernelServices {
            capture: live.output.as_mut(),
            echo: live.echo,
            random: &mut live.random,
            fp_in_cpu: false,
        };
        fault.report(&mut services, live.cell.id);
        self.end(gang, cpu, Ending::Fault);
        false
    }

    /// Counts `ns` of CPU time this CPU ran the cell's threads; ends the
    /// cell if that used up its quota (design D2 of `add-cell-quotas`).
    fn charge_cpu(&mut self, gang: &Gang, cpu: usize, ns: u64) {
        if self.live.cell.quota.cpu_ns == u64::MAX {
            return; // counted only against a quota
        }
        if !self.live.cell.charge_cpu(ns) && self.ending.is_none() {
            out_of_cpu(&mut self.live);
            self.end(gang, cpu, Ending::OutOfCpu);
        }
    }

    /// Records in `Shared::gangs` what the cell offers other CPUs, if that
    /// changed, and wakes idle CPUs for it (design D5).
    fn offer(&mut self, shared: &Global<Shared>, ptr: GangPtr) {
        // Nothing for other CPUs once the cell ends, or while the host waits
        // for every CPU to leave it.
        let offer = if self.ending.is_some() || claimed(self.live.cell.id) {
            (0, None)
        } else {
            let busy = &self.busy;
            let threads = &self.live.cell.threads;
            let spare = threads
                .iter()
                .filter(|t| t.wait == Wait::Ready && !busy.contains(&t.tid))
                .count() as u32;
            (spare, threads.wake_at())
        };
        if offer == self.offer {
            return;
        }
        let new_deadline = offer.1.is_some() && offer.1 != self.offer.1;
        self.offer = offer;
        let wake = shared.with(|s| {
            s.gangs.offer(ptr, offer.0, offer.1);
            // As many idle CPUs as threads offered; one for a new deadline,
            // so that it waits no longer than that.
            let wanted = if offer.0 > 0 {
                offer.0
            } else {
                new_deadline as u32
            };
            let (mut idle, mut wake) = (s.idle, 0u32);
            for _ in 0..wanted {
                let lowest = idle & idle.wrapping_neg();
                wake |= lowest;
                idle &= !lowest;
            }
            wake
        });
        for other in 0..crate::smp::count() {
            if wake & (1 << other) != 0 {
                crate::smp::wake(other);
            }
        }
    }
}

/// Turns `live`, taken by `cpu`, into a gang that other CPUs may join, and
/// runs its threads; `keep` keeps its current thread on this CPU (it just
/// made another).
pub(super) fn start_gang(shared: &Global<Shared>, cpu: usize, live: Box<Live>, keep: bool) {
    let gang = Box::new(Gang {
        id: live.cell.id,
        root: live.cell.root,
        tag: live.tag,
        clean: AtomicU32::new(live.clean),
        ending: AtomicBool::new(false),
        hold: AtomicBool::new(false),
        cell: Global::new(GangCell {
            live,
            busy: [0; MAX_CPUS],
            ending: None,
            offer: (0, None),
        }),
    });
    let id = gang.id;
    let ptr = GangPtr(NonNull::from(Box::leak(gang)), id);
    shared.with(|s| s.gangs.start(cpu, ptr));
    // SAFETY: just made from a box; only the last CPU to leave frees it.
    let gang = unsafe { ptr.0.as_ref() };
    let mut regs = Registers::default();
    let first = gang.locked(shared, ptr, |cell| {
        if !keep {
            return cell.choose(cpu, now_ns(), &mut regs);
        }
        let current = cell.live.cell.threads.current();
        let tid = current.tid;
        regs = current.regs;
        cell.busy[cpu] = tid;
        cell.live.last = tid;
        cell.count_busy();
        Some(tid)
    });
    run_gang(shared, cpu, ptr, first, regs);
}

/// Runs a thread of `ptr`, which `cpu` just joined (design D5): the one it
/// offered, unless another CPU took it meanwhile.
pub(super) fn join_gang(shared: &Global<Shared>, cpu: usize, ptr: GangPtr) {
    // SAFETY: `cpu` is counted in `Shared::gangs` for this gang (it just
    // joined), so the gang stays alive until it leaves.
    let gang = unsafe { ptr.0.as_ref() };
    let mut regs = Registers::default();
    let first = gang.locked(shared, ptr, |cell| {
        // `Gangs::join` took one offered thread (or the deadline).
        match &mut cell.offer {
            (spare @ 1.., _) => *spare -= 1,
            (_, wake) => *wake = None,
        }
        cell.choose(cpu, now_ns(), &mut regs)
    });
    run_gang(shared, cpu, ptr, first, regs);
}

/// Runs threads of the gang `ptr` on `cpu`, `first` (whose registers are
/// `regs`) first, until this CPU leaves the cell: its thread blocks and no
/// other is left for it, yields, or its slice ends, or the cell ends
/// (design D1 of `add-parallel-threads`).
fn run_gang(
    shared: &Global<Shared>,
    cpu: usize,
    ptr: GangPtr,
    first: Option<u32>,
    mut regs: Registers,
) {
    // SAFETY: `cpu` is counted in `Shared::gangs` for this gang until
    // `leave_gang` below, and only the last CPU to leave frees it.
    let gang = unsafe { ptr.0.as_ref() };
    // As in `run_slice`: the first tick may predate the slice.
    let mut ticks = 0;
    let start = now_ns();
    let mut next = first;
    while let Some(tid) = next.take() {
        let key = fp_key(gang.id, tid);
        fp_enter(key, &regs);
        next = loop {
            if claimed(gang.id) {
                // The host waits for the cell: this CPU leaves it.
                gang.locked(shared, ptr, |cell| cell.put_back(cpu, tid, Some(&regs)));
                break None;
            }
            let Some(trap) = gang.enter(cpu, &mut regs) else {
                break None;
            };
            match trap {
                Trap::Syscall => {
                    match gang.locked(shared, ptr, |cell| cell.syscall(gang, cpu, tid, &mut regs)) {
                        After::Resume => continue,
                        After::Switch(next) => break next,
                        After::Leave => break None,
                    }
                }
                Trap::FpUnavailable => fp_first_use(key, &mut regs),
                Trap::Sample => cost::sample(gang.id, regs.pc),
                Trap::Interrupt => {
                    if cost::timer_sampling() {
                        cost::sample(gang.id, regs.pc);
                    }
                    ticks += 1;
                    if ticks >= cost::ticks_per_slice() && !gang.ending.load(Ordering::Acquire) {
                        // The end of this CPU's slice: it looks at the run
                        // queues again (a waiting cell goes first).
                        gang.locked(shared, ptr, |cell| cell.put_back(cpu, tid, Some(&regs)));
                        break None;
                    }
                }
                // Under the cell lock: no other CPU changes its tables.
                Trap::Fault(fault)
                    if gang.locked(shared, ptr, |_| {
                        let pt = PageTable::<Format>::from_root(gang.root);
                        fault.emulate(&mut regs, |pc| instruction(&pt, pc))
                    }) => {}
                Trap::Fault(fault) => {
                    if !gang.locked(shared, ptr, |cell| cell.fault(gang, cpu, &fault, regs.pc)) {
                        break None;
                    }
                }
            }
        };
    }
    // Every CPU's time in the cell counts (design D2 of `add-cell-quotas`).
    let used = now_ns().saturating_sub(start);
    gang.locked(shared, ptr, |cell| cell.charge_cpu(gang, cpu, used));
    leave_gang(shared, cpu, ptr);
}

/// `cpu` stops running the gang `ptr`; the last CPU to leave turns it back
/// into a cell and gives it back (filed as its threads say, or ended).
fn leave_gang(shared: &Global<Shared>, cpu: usize, ptr: GangPtr) {
    // SAFETY: as in `run_gang`, `cpu` has not left yet.
    let id = unsafe { ptr.0.as_ref() }.id;
    if fp_owner() >> 32 == id as u64 {
        set_fp_owner(FP_STALE); // the cell ended: its FP/SIMD state is moot
    }
    user::leave_user();
    // The last CPU to leave still runs the cell until it gives it back.
    let last = shared.with(|s| {
        let last = s.gangs.leave(cpu, ptr);
        if !last {
            s.on_cpu[cpu] = 0;
        }
        last
    });
    if !last {
        return;
    }
    // SAFETY: `ptr` came from `Box::leak` in `start_gang`, and this was the
    // last CPU counted in `Shared::gangs` for it: removed from there under
    // the lock, no other CPU can reach it any more.
    let gang = unsafe { Box::from_raw(ptr.0.as_ptr()) };
    let Gang { cell, clean, .. } = *gang;
    let GangCell {
        mut live, ending, ..
    } = cell.into_inner();
    live.clean = clean.into_inner();
    give_back(shared, cpu, live, ending.map_or(Back::File, Back::End));
}
