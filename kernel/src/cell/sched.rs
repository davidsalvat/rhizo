//! The scheduler loop every CPU runs: taking a cell, giving it back,
//! waking sleepers and idling until the next deadline.

use super::*;

/// What the CPUs running a host's cells share (design D3 of
/// `add-smp-scheduling`), behind one lock taken only to take a cell, give
/// one back, or find, take or put back a cell for a host operation. A cell
/// taken out runs on one CPU (or a gang), which owns it until it gives it
/// back.
#[derive(Debug)]
pub(super) struct Shared {
    /// The live cells that are not running: one run queue per CPU, and the
    /// sleepers (spec `scheduling`); round robin, or by weight once the
    /// tree needs it, and within exclusive CPU sets once a domain has one
    /// ([`Host::reshare`]).
    pub(super) sched: Scheduler<Box<Live>, Option<Fair>>,
    /// When each CPU took the cell it runs: the start of the slice charged
    /// to it while cells share by weight.
    pub(super) taken: [u64; MAX_CPUS],
    pub(super) tags: TagPool,
    /// Cells that ended since the host last looked ([`Host::collect`]).
    pub(super) ended: Vec<Finished>,
    /// Cells taken out of `sched`, running on a CPU.
    pub(super) running: usize,
    /// CPUs waiting for work in the current run (bit `n` for CPU `n`).
    pub(super) idle: u32,
    /// The current run is over: CPUs give their cells back and stop.
    pub(super) stopping: bool,
    /// Cells woken by a socket change while they were not in the waiting
    /// set (running, or about to be filed): their threads waiting on
    /// descriptors look again when they are next taken or given back
    /// (design D2 of `add-socket-wait-queues`).
    pub(super) nudged: BTreeSet<u32>,
    /// Cells taken out whose threads run on several CPUs (design D1 of
    /// `add-parallel-threads`).
    pub(super) gangs: Gangs<GangPtr>,
    /// The idle CPU whose timer covers the earliest deadline (design D1 of
    /// `add-tickless-idle`).
    pub(super) timer: IdleTimer,
    /// The cell each CPU runs, alone or as part of a gang (0: none): the
    /// CPUs a host operation on it stops (design D2 of `host-ops-per-cell`).
    pub(super) on_cpu: [u32; MAX_CPUS],
    /// The cell [`CLAIM`] names, given back by the last CPU that ran it.
    pub(super) claimed: Option<Box<Live>>,
}

/// The cell a host operation waits for while a CPU runs it (0: none): the
/// CPUs running it give it back at their next interrupt, into
/// [`Shared::claimed`] instead of the scheduler (design D2 of
/// `host-ops-per-cell`). Cell identifiers are never reused, so one word
/// serves every host.
pub(super) static CLAIM: AtomicU32 = AtomicU32::new(0);

/// Whether a host operation waits for cell `id`.
pub(super) fn claimed(id: u32) -> bool {
    CLAIM.load(Ordering::Relaxed) == id
}

/// Wakes the CPUs in `mask` (bit `n` for CPU `n`).
pub(super) fn wake_cpus(mask: u32) {
    if mask == 0 {
        return;
    }
    for cpu in 0..crate::smp::count() {
        if mask & (1 << cpu) != 0 {
            crate::smp::wake(cpu);
        }
    }
}

/// What a CPU does next ([`take`]).
pub(super) enum Take {
    /// Run this cell, picked at this time.
    Run(Box<Live>, u64),
    /// Run a thread of this running cell.
    Join(GangPtr),
    /// Wait for work until this time (or a wake-up, or for ever).
    Wait(Option<u64>),
    /// Cells ended that the boot CPU has not looked at: look first (its
    /// stop condition may hold now).
    Look,
    /// No cell can ever run again in this run.
    Done,
    /// The run is over.
    Stop,
}

/// How a cell comes back from its slice ([`give_back`]).
#[derive(Clone, Copy)]
pub(super) enum Back {
    /// Preempted or yielding: runnable again.
    Requeue,
    /// Its threads block: runnable or asleep, as they say.
    File,
    /// It ended.
    End(Ending),
    /// Its thread made another: other CPUs may run them (design D2 of
    /// `add-parallel-threads`).
    Share,
}

/// The lowest idle secondary CPU if a cell is runnable (bit `n` for CPU
/// `n`; 0 for none, as between runs): the boot CPU, working for the host,
/// leaves runnable cells to it. With exclusive sets, one for each class of
/// CPUs that has runnable cells.
pub(super) fn idle_for_runnable(s: &Shared) -> u32 {
    let others = s.idle & !1;
    if s.sched.sets().is_some() {
        return s.sched.wanting(others.into(), None) as u32;
    }
    if others != 0 && s.sched.runnable() > 0 {
        others & others.wrapping_neg()
    } else {
        0
    }
}

/// The CPUs that run cell `id` (bit `n` for CPU `n`).
pub(super) fn running_on(s: &Shared, id: u32) -> u32 {
    (0..MAX_CPUS)
        .filter(|&cpu| s.on_cpu[cpu] == id)
        .fold(0, |mask, cpu| mask | 1 << cpu)
}

/// Takes the next cell for `cpu`: the first of its queue or one stolen
/// (`oldest`: the one that has waited longest anywhere, for a single CPU
/// stepping through every cell).
pub(super) fn take(shared: &Global<Shared>, cpu: usize, oldest: bool) -> Take {
    let _p = cost::scope(Part::Sched);
    let _timed = prof::Timed::new(Count::SchedCycles);
    shared.with(|s| {
        if s.stopping {
            s.idle &= !(1 << cpu);
            return Take::Stop;
        }
        let now = now_ns();
        wake_woken(s, cpu, || now);
        if prof::on() {
            // When each due cell joins a queue, for its latency.
            let q = if oldest { 0 } else { cpu };
            s.sched
                .wake_due(q, now, |live| live.ready = live.ready.due(now));
        }
        let next = if oldest {
            s.sched.next_oldest(now)
        } else {
            s.sched.next(cpu, now)
        };
        if s.sched.sets().is_some() {
            wake_classes(s, cpu);
        }
        if let Some(mut live) = next {
            nudge(s, &mut live);
            live.ready.ran(now);
            live.ready = Ready::default();
            s.running += 1;
            s.on_cpu[cpu] = live.cell.id;
            s.taken[cpu] = now;
            s.idle &= !(1 << cpu);
            if s.idle != 0 || s.timer.armed().is_some() {
                cover(s, cpu);
            }
            return Take::Run(live, now);
        }
        if !oldest && let Some(gang) = join(s, cpu, now) {
            s.on_cpu[cpu] = gang.1;
            s.taken[cpu] = now; // the last CPU to leave gives the cell back
            s.idle &= !(1 << cpu);
            if s.idle != 0 || s.timer.armed().is_some() {
                cover(s, cpu);
            }
            return Take::Join(gang);
        }
        // The boot CPU looks at ended cells before it waits: with nothing
        // to run its wait may last until the network card wants polling,
        // hours away, and the host would not see the cell end.
        if cpu == 0 && !s.ended.is_empty() {
            return Take::Look;
        }
        let wake = next_wake(s);
        // Under the lock: a CPU giving a cell back sees this bit and wakes
        // this one, so no wake-up is lost between here and the wait.
        s.idle |= 1 << cpu;
        // A timer it held covers nothing any more: it looks again.
        s.timer.leave(cpu);
        if s.running == 0 && prof::on() {
            prof::nothing_runs(now);
        }
        if s.running == 0 && wake.is_none() {
            // Only parked cells are left: nothing will wake them (but the
            // network card, or the end of the run).
            return Take::Done;
        }
        // Its timer only if no other idle CPU's covers the deadline.
        Take::Wait(s.timer.idle(cpu, wake))
    })
}

/// With exclusive CPU sets (design D17 of `add-cell-hierarchy`), the
/// cells `cpu` just woke may run only on another class of CPUs: wakes the
/// lowest idle CPU of each class with runnable cells. Out of line: without
/// sets, taking a cell costs a test.
#[cold]
#[inline(never)]
fn wake_classes(s: &Shared, cpu: usize) {
    let idle = u64::from(s.idle & !(1 << cpu));
    wake_cpus(s.sched.wanting(idle, None) as u32);
}

/// A gang `cpu` joins at `now`: one of a cell it may run (design D17 of
/// `add-cell-hierarchy`).
fn join(s: &mut Shared, cpu: usize, now: u64) -> Option<GangPtr> {
    if s.sched.sets().is_none() {
        return s.gangs.join(cpu, now);
    }
    let sched = &s.sched;
    s.gangs
        .join_where(cpu, now, |g| sched.allowed(g.1) & 1 << cpu != 0)
}

/// The earliest deadline of the cells that are not running.
fn next_wake(s: &Shared) -> Option<u64> {
    match (s.sched.next_wake(), s.gangs.next_wake()) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
    .filter(|&t| t != u64::MAX)
}

/// `cpu` took work: wakes an idle CPU so that its timer covers the
/// earliest deadline, if none does (design D1 of `add-tickless-idle`).
/// Under the host's lock: rare, only when a deadline loses its timer. Out
/// of line, so that taking a cell with no CPU idle costs a test.
#[cold]
#[inline(never)]
fn cover(s: &mut Shared, cpu: usize) {
    if s.idle == 0 {
        // No CPU idles: busy CPUs look at every slice's end. Only an idle
        // CPU holds the timer, so it was this one, if any.
        s.timer = IdleTimer::new();
        return;
    }
    s.timer.leave(cpu);
    let next = next_wake(s);
    if let Some(other) = s.timer.cover(next, s.idle) {
        crate::smp::wake(other);
    }
}

/// Wakes the cells whose sockets changed since the last call (design D2 of
/// `add-socket-wait-queues`): a waiting one joins `cpu`'s queue and makes
/// its calls again; any other is nudged ([`Shared::nudged`]). Called under
/// the host's lock, so a cell filed to wait cannot miss its wake-up.
fn wake_woken(s: &mut Shared, cpu: usize, now: impl FnOnce() -> u64) {
    // Set under the sockets' lock after a change woke waiters: clear, it
    // means nothing waits to be drained (a change after this load sets it
    // again, for the next drain, which comes before its cell is filed).
    if !WOKEN.swap(false, Ordering::Acquire) {
        return;
    }
    let now = now();
    // Lock order: the host's, then the sockets.
    NET.with(|net| {
        net.drain_woken(|id| {
            let waited = s.sched.wake(cpu, id, |live| {
                live.cell.threads.wake_io();
                live.ready = live.ready.net_change(now);
                live.io_woken = prof::on();
            });
            if !waited {
                s.nudged.insert(id);
            }
        })
    });
}

/// Lets the threads of `live` waiting on descriptors look again if a
/// socket they waited on changed while the cell was out of the waiting set.
fn nudge(s: &mut Shared, live: &mut Live) {
    if !s.nudged.is_empty() && s.nudged.remove(&live.cell.id) {
        live.cell.threads.wake_io();
    }
}

/// Waits for work: until `wake`, or a wake-up interrupt. Without `wake`,
/// the CPU's timer is off for the wait (design D2 of `add-tickless-idle`):
/// another idle CPU's timer covers the earliest deadline, or there is none.
pub(super) fn wait(wake: Option<u64>) {
    let _p = cost::scope(Part::Idle);
    match wake {
        Some(at) => idle_until(at),
        None => {
            irq::stop_tick();
            prof::idle(crate::smp::idle);
            irq::start_tick();
        }
    }
    cost::after_idle();
}

/// Puts `live`, back from a slice on `cpu`, where `how` says, and wakes an
/// idle CPU that has something to do because of it: another runnable cell
/// to take, or the boot CPU to see a cell end or the last one stop.
pub(super) fn give_back(shared: &Global<Shared>, cpu: usize, mut live: Box<Live>, how: Back) {
    let _p = cost::scope(Part::Sched);
    let _timed = prof::Timed::new(Count::SchedCycles);
    // The cell back, and whether it is filed (else requeued).
    let (ended, back) = match how {
        Back::End(ending) => {
            shared.with(|s| s.nudged.remove(&live.cell.id));
            (Some(end(live, ending)), None)
        }
        Back::Requeue => {
            live.ready = ready_now();
            (None, Some((live, false)))
        }
        Back::File | Back::Share => (None, Some((live, true))),
    };
    let was_end = ended.is_some();
    // One critical section puts the cell back and does the bookkeeping.
    let wake = shared.with(|s| {
        if let Some((live, filed)) = back {
            // A requeued cell's woken cells and nudge wait for the next take.
            if filed {
                wake_woken(s, cpu, now_ns);
            }
            let taken = s.taken[cpu];
            if claimed(live.cell.id) || filed {
                if s.sched.shares().is_some() {
                    charge(s, cpu, live.cell.id);
                }
                if claimed(live.cell.id) {
                    s.claimed = Some(live);
                } else {
                    file(s, Some(cpu), live);
                }
            } else {
                s.sched.requeue_charged(cpu, live, || slice(taken));
            }
        }
        s.running -= 1;
        s.on_cpu[cpu] = 0;
        if let Some((finished, tag)) = ended {
            if tag != 0 {
                s.tags.free(tag);
            }
            s.ended.push(finished);
        }
        let others = s.idle & !(1 << cpu);
        if s.sched.sets().is_some() {
            within_classes(s, cpu, others, was_end)
        } else if s.sched.runnable() > 1 && others != 0 {
            others & others.wrapping_neg() // the lowest idle CPU
        } else if was_end || (s.running == 0 && s.sched.runnable() == 0) {
            others & 1 // the boot CPU checks whether the run is over
        } else {
            0
        }
    });
    wake_cpus(wake);
}

/// [`give_back`]'s CPUs to wake with exclusive CPU sets (design D17 of
/// `add-cell-hierarchy`), among the idle CPUs `others`: the lowest of each
/// class with a runnable cell `cpu` does not take next, and the boot CPU
/// if a cell ended or nothing is left.
#[cold]
#[inline(never)]
fn within_classes(s: &Shared, cpu: usize, others: u32, was_end: bool) -> u32 {
    let wake = s.sched.wanting(others.into(), Some(cpu)) as u32;
    let over = was_end || (s.running == 0 && s.sched.runnable() == 0);
    wake | if over { others & 1 } else { 0 }
}

/// The length and end of a slice taken at `taken`, ending now.
fn slice(taken: u64) -> (u64, u64) {
    let now = now_ns();
    (now.saturating_sub(taken), now)
}

/// Charges cell `id` the slice it just ran on `cpu`, before it is put back
/// (design D23 of `add-cell-hierarchy`): a subtree past its CPU cap leaves
/// the run queues until its period ends. Only while cells share by weight.
#[inline(never)]
fn charge(s: &mut Shared, cpu: usize, id: u32) {
    let (ns, now) = slice(s.taken[cpu]);
    s.sched.charge(cpu, id, ns, now);
}

/// Files a cell whose threads may all wait: runnable if one of its threads
/// can run (back on `cpu`'s queue, or on the shortest for a cell new to the
/// scheduler), else asleep until its earliest deadline, or parked for good
/// when none of its threads has one (design D2 of `add-threads`). A cell
/// waiting on descriptors waits for a socket it looked at to change (design
/// D2 of `add-socket-wait-queues`).
pub(super) fn file(s: &mut Shared, cpu: Option<usize>, mut live: Box<Live>) {
    nudge(s, &mut live);
    if cpu.is_none() {
        // New to the scheduler (forked, resumed, woken by the host), it is
        // on no wait queue yet: its threads make their calls again.
        live.cell.threads.wake_io();
    }
    let sched = &mut s.sched;
    if live.cell.threads.is_runnable() {
        live.ready = ready_now();
        match cpu {
            Some(cpu) => sched.requeue(cpu, live),
            None => sched.place(live),
        };
    } else if live.cell.threads.waits_on_io() {
        // Woken by a socket change or its earliest deadline.
        let wake = live.cell.threads.wake_at();
        live.ready = Ready::event(wake.unwrap_or(0));
        sched.wait(live.cell.id, live, wake);
    } else {
        let wake = live.cell.threads.wake_at().unwrap_or(u64::MAX);
        live.ready = Ready::event(if wake == u64::MAX { 0 } else { wake });
        sched.sleep(live, wake);
    }
}

/// Runnable since now, for the workloads' latencies (nothing while they
/// are not measured: no clock read).
fn ready_now() -> Ready {
    if prof::on() {
        Ready::queued(now_ns())
    } else {
        Ready::default()
    }
}

/// Ends `live`: drops the CPU's hold on its address space and FP/SIMD
/// state, frees its memory, and returns what the host keeps of it and the
/// tag to free. Then gives the kernel heap's free pages back if enough of
/// its allocations went since its last trim (design D2 of `add-heap-trim`).
pub(super) fn end(live: Box<Live>, ending: Ending) -> (Finished, u16) {
    let _p = cost::scope(Part::Teardown);
    let ended = {
        let live = *live;
        if fp_owner() >> 32 == live.cell.id as u64 {
            set_fp_owner(FP_STALE);
        }
        user::release_user(live.cell.root, live.tag);
        sharing(live.shares, || {
            space::destroy(
                PageTable::<Format>::from_root(live.cell.root),
                &mut DirectMap,
                &mut KernelFrames,
            )
        });
        release_sockets(&live.fs, live.cell.id);
        let finished = Finished {
            id: live.cell.id,
            ending,
            output: live.output.unwrap_or_default(),
            cpus: live.cpus,
            peak: live.peak,
        };
        (finished, live.tag)
    };
    crate::heap::trim_if_due();
    ended
}

/// What a secondary CPU runs during [`Host::run`]: cells from the shared
/// scheduler until the run stops, with its preemption tick on.
pub(super) fn run_cpu(shared: u64) -> u64 {
    // SAFETY: `Host::run` passes the address of its `shared`, and waits for
    // this function to return before it returns itself, so the reference is
    // valid for the whole call; `Global` serializes access.
    let shared = unsafe { &*(shared as *const Global<Shared>) };
    let cpu = crate::arch::cpu::index();
    irq::start_tick();
    loop {
        match take(shared, cpu, false) {
            Take::Run(live, now) => run_slice(shared, cpu, live, now, true),
            Take::Join(gang) => join_gang(shared, cpu, gang),
            Take::Wait(wake) => wait(wake),
            Take::Done | Take::Look => wait(None),
            Take::Stop => break,
        }
    }
    irq::stop_tick();
    0
}

/// The CPUs whose TLB holds nothing stale for a cell once this CPU has
/// flushed (all of them when flushes are broadcast, design D5).
pub(super) fn flushed_on(cpu: usize) -> u32 {
    if user::TLB_FLUSH_IS_BROADCAST {
        u32::MAX
    } else {
        1 << cpu
    }
}

/// The CPUs whose TLB holds nothing stale for a cell once the kernel,
/// running on this CPU, flushed some of its translations: none with KPTI,
/// where the kernel's flushes cannot reach the cell's PCID (design D3 of
/// `add-kpti`), so each CPU flushes it on its next entry.
pub(super) fn flushed_by_kernel_on(cpu: usize) -> u32 {
    if user::stale_everywhere() {
        0
    } else {
        flushed_on(cpu)
    }
}

/// Waits without running anything until monotonic time `wake`.
pub(super) fn idle_until(wake: u64) {
    let now = now_ns();
    if wake <= now {
        return;
    }
    if irq::available() {
        prof::idle(|| irq::wait_ns(wake - now));
    } else {
        while now_ns() < wake {
            core::hint::spin_loop();
        }
    }
}
