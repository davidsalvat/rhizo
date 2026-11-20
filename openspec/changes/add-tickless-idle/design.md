# Design

## D1. One idle CPU holds the timer for the earliest deadline

`rhizo_sched::IdleTimer`, kept in the host's shared state next to the run
queues and the idle CPUs, records which idle CPU's timer covers which
deadline. Under the scheduler's lock, in `take`:

- a CPU that looks for work first drops the timer it held (`leave`): it is
  awake, its timer covers nothing any more;
- a CPU that goes idle while `next` is the earliest deadline of the cells
  that are not running (sleepers, waiters with a deadline, threads of cells
  on several CPUs) programs its timer for `next` only when no other idle
  CPU's timer fires by then (`idle`); otherwise it waits for a wake-up
  interrupt only;
- a CPU that takes work (a cell, or threads of a running cell) while some
  CPU idles checks that an idle CPU's timer covers `next` (`cover`); if
  none does, it wakes one: the CPU holding a later timer first, so that no
  CPU keeps a timer nobody needs, else the lowest idle CPU. That CPU counts
  as covering `next` until it looks again, so other CPUs do not wake it
  twice; when it looks it either finds work (and hands over in turn) or
  programs its timer.

This covers the two ways a deadline loses its timer: the CPU holding it is
woken for work, or a busy CPU files a cell with an earlier deadline (it
calls `take` right after). A deadline reached while every CPU runs is seen
at the next scheduling decision of any of them, within a slice, as before.
`IdleTimer` reads no clock and is tested on the host, including a property
test over any interleaving of four CPUs: while a CPU idles and a deadline
exists, an idle CPU's timer fires by then or an idle CPU was woken. It
fails if `leave` or `cover` does nothing.

A CPU with nothing to run and no deadline anywhere (`Take::Done`) now also
counts as idle, so a CPU giving back a cell wakes it for work as it wakes
the others; before, only the slice-long poll brought it back.

Alternatives: every idle CPU programs its timer for the earliest deadline
(every idle CPU wakes at every deadline, and all but one find nothing);
per-CPU sleep lists, each CPU responsible for the deadlines of the cells it
filed (a CPU that keeps running must then hand its sleepers over, and
stealing a due sleeper needs another CPU's list); keeping the safety-net
poll with a longer period (still wake-ups, and a latency as long as the
period).

## D2. No tick while idle

An idle CPU without a deadline stops its timer before it waits and starts
the periodic tick again after (`irq::stop_tick`, `irq::start_tick`); before,
on both architectures, a secondary CPU waiting for a wake-up during a run
still took every tick. A timed wait (`irq::wait_ns`) now always restarts the
periodic tick when it ends: on AArch64 a wait ended by a wake-up interrupt
left the wait's timer armed, so the next cell would have been preempted only
at that deadline, seconds later once idle waits are no longer capped at a
slice. `CNTV_TVAL_EL0` holds a signed 32-bit value: longer waits are cut to
`i32::MAX` counter ticks (34 s at 62.5 MHz) instead of firing at once; the
CPU then looks again and waits for the rest. The x86_64 one-shot count
already holds 68 s.

## D3. What still wakes an idle CPU

- Its timer, when it holds the earliest deadline (one CPU at a time).
- A wake-up interrupt: work given back or offered by another CPU (as
  before), a deadline handed over (D1), the end of a run.
- On the boot CPU, the network card's polling while cells wait on
  descriptors (design D4 of `add-virtio-net`), until card interrupts come.

## D4. Tests and metric

`idle_cpus_wake_only_for_deadlines`: a cell sleeps 300 ms while every CPU
idles; the CPUs leave their waits at most four times per CPU in all (124
times on 4 vCPUs before this change), and the sleeper ends within 400 ms.
The rest of the suite on 4 vCPUs (`ktest-smp`) checks that no wake-up is
lost: every multi-CPU test sleeps, waits on futexes and sockets, and ends.
The metric is `laptop.idle_wakeups_per_s`, already gated in the `ci`
environment; its baseline drops with this change.
