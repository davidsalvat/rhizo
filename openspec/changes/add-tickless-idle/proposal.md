# Proposal

## Why

The laptop reference workload measures how often the guest's CPUs leave
their waits while every agent idles: the vision's laptop goal is "near-zero
CPU and power with idle cells". With one CPU the kernel already sleeps until
the next deadline (about one wake-up per second). With more than one, every
idle CPU looks again at least every slice (10 ms) whenever any cell sleeps,
a safety net for events that happen on another CPU: about 100 wake-ups per
second per CPU (199 per second on 2 vCPUs once `fix-secondary-cpu-timer`
gave every CPU the right timer rate), and the VMM's threads wake as often.

## What Changes

- Tickless idle with several CPUs: of the idle CPUs, one at most programs
  its timer, for the earliest deadline of the cells that are not running;
  the others wait with their timer off, for a wake-up interrupt only. No
  idle CPU has a periodic tick.
- Deadlines are never left uncovered while a CPU idles: a CPU that takes
  work while the earliest deadline is not covered by an idle CPU's timer
  (it held that timer itself, or it just filed a cell with an earlier
  deadline) wakes an idle CPU, which takes the deadline over.
- A wait on a CPU's timer always goes back to the periodic tick when it
  ends, whatever ended it (on AArch64 a wait ended by a wake-up left its
  long timer to preempt the next cell), and a long wait is cut to what the
  AArch64 timer register holds instead of firing at once.
- The `multi-core` requirement on idle CPUs states it; the logic is a
  host-tested part of `rhizo-sched` (`IdleTimer`).

### Out of scope

Waking idle CPUs on network-card interrupts, and per-socket wait queues
(another change: the boot CPU still polls the card while cells wait on
descriptors); a socket change made by a cell on a busy CPU is still seen at
that CPU's next scheduling decision, within one slice, as before.
Address-space tags running out (its own change).

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `multi-core`: idle CPUs keep one timer between them, for the earliest
  deadline, and none has a tick.

## Impact

`crates/rhizo-sched/src/idle.rs` (new), `kernel/src/cell.rs` (`take`,
`wait`, `Host::run`), `kernel/src/arch/aarch64/irq.rs` (`wait_ns`, the
timer's range), `kernel/src/tests.rs`, `bench/README.md`,
`bench/baseline.json` (the laptop's idle wake-ups drop).
