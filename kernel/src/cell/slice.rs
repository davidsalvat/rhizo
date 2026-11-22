//! One time slice of a cell on one CPU: its threads switching inside
//! the slice, their system calls, and the CPU quota.

use super::*;

/// Runs the system call of `live`'s current thread, whose FP/SIMD state is
/// in this CPU's registers when `fp_in_cpu`, through the TLB of `gang` when
/// other CPUs may run the cell's code. Returns what the thread does next
/// and whether the cell's translations shrank. Inlined: it is the hot path
/// of `run_slice`, as it was before it was shared with gangs.
#[inline(always)]
pub(super) fn syscall(
    live: &mut Live,
    pt: PageTable<Format>,
    fp_in_cpu: bool,
    gang: Option<(&Gang, usize)>,
) -> (Outcome, bool) {
    let tid = live.cell.threads.current().tid;
    let deadline = live.take_deadline(tid);
    let mut services = KernelServices {
        capture: live.output.as_mut(),
        echo: live.echo,
        random: &mut live.random,
        fp_in_cpu,
    };
    let (mut tlb, mut pager_tlb) = (CellTlb::new(gang), CellTlb::new(gang));
    let t0 = prof::syscall_start();
    // Lock order: the cell's (when shared), `SHARING`, then the sockets.
    let (outcome, deadline) = sharing(live.shares, || {
        let _call = cost::syscall(&live.cell);
        let mut sockets = CallNet::default();
        let mut cx = Context {
            pt,
            mem: &mut DirectMap,
            tlb: &mut tlb,
            frames: &mut cell_frames(&live.frames),
            services: &mut services,
            fs: &mut live.fs,
            net: &mut sockets,
            deadline,
            pager: &mut CellPager(&mut live.demand, &mut pager_tlb, &live.frames),
            pages: &mut KernelPages,
        };
        let outcome = live.abi.handle(&mut live.cell, &mut cx);
        let deadline = cx.deadline;
        let waits = || matches!(live.cell.threads.current().wait, Wait::Io { .. });
        if let Some((net, changes)) = sockets.0.as_mut() {
            // A call that waits on descriptors joins the wait queues of the
            // sockets it looked at (design D1 of `add-socket-wait-queues`),
            // under the lock it looked at them with.
            if net.end_call(|| waits().then_some(live.cell.id)) {
                WOKEN.store(true, Ordering::Release);
            }
            if prof::on() && net.changes() != *changes {
                prof::net_changed(now_ns());
            }
        }
        if prof::on() && core::mem::take(&mut live.io_woken) && waits() {
            prof::add(Count::IoWasted, 1);
        }
        (outcome, deadline)
    });
    prof::syscall_end(t0);
    // Only a call that blocked can have started the wait the wake-up is
    // kept for.
    if matches!(outcome, Outcome::Block) && live.host_wake {
        live.keep_host_wake();
    }
    // A call waiting on descriptors keeps its deadline for when it is made
    // again (design D3 of `add-sockets-and-poll`).
    if let Wait::Io { until: Some(_) } = live.cell.threads.current().wait
        && let Some(at) = deadline
    {
        live.deadlines.push((tid, at));
    }
    (outcome, tlb.shrank || pager_tlb.shrank)
}

/// Whether calls of `live`'s threads woke threads of their own cell waiting
/// on descriptors (two threads talking over loopback, design D5 of
/// `add-cell-hierarchy`): then they are woken here, to run in this slice,
/// instead of the cell going through the scheduler, which would wake them
/// all the same. Asked only when no thread of the cell can run.
fn woke_itself(live: &mut Live) -> bool {
    if !WOKEN.load(Ordering::Acquire) || !NET.with(|net| net.take_woken(live.cell.id)) {
        return false;
    }
    live.cell.threads.wake_io()
}

impl Live {
    /// A [`Host::wake`] came before any thread waited with a deadline: the
    /// first wait with one ends at once (design D5 of
    /// `add-socket-wait-queues`).
    #[cold]
    #[inline(never)]
    fn keep_host_wake(&mut self) {
        if self.cell.threads.wake_at().is_some() {
            self.host_wake = false;
            self.expire_timers();
        }
    }
}

/// Runs `live`'s threads on `cpu` until the cell ends, yields, has no
/// thread that can run, or is preempted, and gives it back. With `share`
/// (several CPUs, not stepping), a cell with several threads runs as a
/// [`Gang`] instead, which other CPUs may join.
pub(super) fn run_slice(
    shared: &Global<Shared>,
    cpu: usize,
    mut live: Box<Live>,
    now: u64,
    share: bool,
) {
    if share && live.cell.threads.len() > 1 {
        start_gang(shared, cpu, live, false);
        return;
    }
    if !live.cell.threads.next(now) {
        give_back(shared, cpu, live, Back::File); // nothing due yet after all
        return;
    }
    live.cpus |= 1 << cpu;
    // The slice's CPU time, for a cell with a CPU quota: its start now, its
    // end added in `check_cpu_quota`, so nothing stays in a register across
    // the slice (design D2 of `add-cell-quotas`).
    if live.cell.quota.cpu_ns != u64::MAX {
        live.cpu_ticks = live.cpu_ticks.wrapping_sub(Counter.cycles());
    }
    let pt = PageTable::<Format>::from_root(live.cell.root);
    // Ticks seen in this slice: the first may have been pending since
    // before the cell was picked, so a cell is preempted at the second,
    // and always makes progress (design D4).
    let mut ticks = 0;
    // The thread running, and whether this CPU's FP/SIMD registers hold its
    // state (design D1 of `lazy-fp-switch`).
    let (mut key, mut fp_mine);
    // Each thread that runs in the slice (design D2 of `add-threads`).
    let back = 'thread: loop {
        key = fp_key(live.cell.id, live.cell.threads.current().tid);
        fp_mine = fp_enter(key, live.cell.regs());
        loop {
            let flush = live.clean & (1 << cpu) == 0;
            let trap = {
                let _p = cost::scope(Part::Cell);
                user::enter_user(live.cell.root, live.tag, flush, live.cell.regs_mut())
            };
            live.clean |= flushed_on(cpu);
            match trap {
                Trap::Syscall => {
                    let (outcome, shrank) = syscall(&mut live, pt, fp_mine, None);
                    if shrank {
                        live.clean = flushed_by_kernel_on(cpu);
                    }
                    match outcome {
                        // A new thread: other CPUs may run it (design D2 of
                        // `add-parallel-threads`).
                        Outcome::Resume if share && live.cell.threads.len() > 1 => {
                            break 'thread Back::Share;
                        }
                        Outcome::Resume => continue,
                        Outcome::Yield => break 'thread Back::Requeue,
                        Outcome::Block => {
                            // Its FP/SIMD state goes to memory (the
                            // registers stay its until another thread
                            // needs them).
                            if fp_mine {
                                fp_save_thread(&mut live, key);
                                fp_mine = false;
                            }
                            // Another thread of the cell runs at once, in
                            // this slice, without changing address space;
                            // not while the host waits for the cell to
                            // pause, which it sees only between slices
                            // (change `fix-warm-wait-race`).
                            if !live.hold
                                && (live.cell.threads.next(now_ns())
                                    || (woke_itself(&mut live) && live.cell.threads.next(now_ns())))
                            {
                                continue 'thread;
                            }
                            break 'thread Back::File;
                        }
                        Outcome::Exit(status) => break 'thread Back::End(Ending::Exit(status)),
                    }
                }
                Trap::FpUnavailable => {
                    fp_first_use(key, live.cell.regs_mut());
                    fp_mine = true;
                    continue;
                }
                Trap::Interrupt => {
                    if cost::timer_sampling() {
                        cost::sample(live.cell.id, live.cell.regs().pc);
                    }
                    prof::add(Count::Interrupts, 1);
                    ticks += 1;
                    // The end of its slice, or the host waits for it.
                    if ticks < cost::ticks_per_slice() && !claimed(live.cell.id) {
                        continue;
                    }
                    break 'thread Back::Requeue;
                }
                Trap::Sample => {
                    cost::sample(live.cell.id, live.cell.regs().pc);
                    continue;
                }
                Trap::Fault(fault)
                    if fault.missing_page().is_some_and(|addr| {
                        live.demand.lazy.as_mut().is_some_and(|l| {
                            let _p = cost::scope(Part::Fault);
                            l.page_in(&pt, &mut cell_frames(&live.frames), addr)
                        })
                    }) =>
                {
                    prof::add(Count::LazyFaults, 1);
                    if live.demand.lazy.as_ref().is_some_and(|l| l.left == 0) {
                        live.demand.lazy = None; // fully restored: drop the snapshot
                    }
                    continue;
                }
                Trap::Fault(fault)
                    if fault.missing_page().is_some_and(|addr| {
                        grow_stack(&pt, &mut cell_frames(&live.frames), addr)
                    }) =>
                {
                    continue;
                }
                Trap::Fault(fault)
                    if fault.write_denied().is_some_and(|addr| {
                        sharing(live.shares, || {
                            resolve_write(&pt, &mut CellTlb::new(None), addr)
                        })
                    }) =>
                {
                    prof::add(Count::CowFaults, 1);
                    live.clean = flushed_by_kernel_on(cpu);
                    continue;
                }
                Trap::Fault(fault)
                    if fault.emulate(live.cell.regs_mut(), |pc| instruction(&pt, pc)) =>
                {
                    continue;
                }
                Trap::Fault(fault) => {
                    if wx_fault(&mut live, &pt, cpu, &fault) {
                        continue;
                    }
                    let mut services = KernelServices {
                        capture: live.output.as_mut(),
                        echo: live.echo,
                        random: &mut live.random,
                        fp_in_cpu: false,
                    };
                    fault.report(&mut services, live.cell.id);
                    break 'thread Back::End(Ending::Fault);
                }
            }
        }
    };
    // CPU time counts for the whole slice, its calls included; a cell past
    // its quota ends at the end of the slice that used it up (design D2 of
    // `add-cell-quotas`). Without a quota, the ticks are only added up.
    let back = if live.cell.quota.cpu_ns == u64::MAX {
        back
    } else {
        check_cpu_quota(&mut live, back)
    };
    if let Back::Share = back {
        // Every thread's FP/SIMD state goes to memory: another CPU may run
        // it next.
        sync_fp(&mut live, true);
        start_gang(shared, cpu, live, true);
        return;
    }
    // An ended cell's state is moot: `end` lets go of it.
    let ended = matches!(back, Back::End(_));
    if !ended && crate::smp::count() > 1 {
        // Another CPU may take the cell next: its FP/SIMD state goes to
        // memory, and this CPU lets go of it and of its tables (design D4).
        sync_fp(&mut live, true);
        user::leave_user();
    } else if !ended && fp_mine {
        // Its FP/SIMD state goes to memory, so the next cell to run here
        // can have the registers (design D1 of `lazy-fp-switch`); they stay
        // its until then, and it needs no load if it runs next.
        fp_save_thread(&mut live, key);
    }
    give_back(shared, cpu, live, back);
}

/// Reports in `live`'s output that it used up its CPU time quota.
/// Counts the CPU time of a cell with a CPU quota at the end of its slice
/// and ends it past its quota (design D2 of `add-cell-quotas`).
#[cold]
#[inline(never)]
fn check_cpu_quota(live: &mut Live, back: Back) -> Back {
    live.cpu_ticks = live.cpu_ticks.wrapping_add(Counter.cycles());
    if !live.count_cpu() && !matches!(back, Back::End(_)) {
        out_of_cpu(live);
        return Back::End(Ending::OutOfCpu);
    }
    back
}

pub(super) fn out_of_cpu(live: &mut Live) {
    use core::fmt::Write;
    let mut services = KernelServices {
        capture: live.output.as_mut(),
        echo: live.echo,
        random: &mut live.random,
        fp_in_cpu: false,
    };
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(&mut services),
        "cell {} killed: CPU time quota used up",
        live.cell.id
    );
}
