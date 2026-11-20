# Proposal

## Why

The server reference workload (`add-reference-workloads`) found the
largest waste in the kernel: every socket change wakes every cell waiting
on any socket (design D3 of `add-sockets-and-poll`, which named per-object
wait queues as the fix "when a benchmark shows it"). In CI, 118,000 socket
changes in 3 s woke 1.7 million cells; each ran, made its call again,
found nothing and waited again: 92 % of the load's cell runs were such re-checks,
scheduling took 35 % of the CPU, and the wake-up latency of the cells that
did have something to do waited behind all the others
(`density.wake_p50_us` around 3 ms, `density.sched_p99_us` around 10 ms,
up to 400 ms on Firecracker). The benchmark shows it.

## What Changes

- **A wait queue per socket.** A system call that waits on descriptors
  joins the wait queue of each socket it looked at (the ones it read,
  wrote, accepted on, connected or polled, directly or through `epoll`).
  A change to a socket (data queued, room for a writer that found its
  peer's queue full, a connection to accept, a close, a reset, a frame from
  the network card for a socket on the wire) wakes only the waiters on its
  queue. Nothing else wakes a waiting cell but its deadline.
- **No lost wake-up.** A cell joins the queues inside the system call that
  found nothing ready, under the same lock as the check. A change that
  comes before the cell is filed as waiting (another CPU) is not lost: the
  cell is nudged and looks again when it is filed or next taken.
- **The waiting set is keyed by cell**, so waking one cell does not scan
  the others; deadlines are kept in order, so the next deadline and the
  due waiters are found without a scan either.
- **Pipes, event counters and `epoll` instances** belong to one cell, so
  only that cell's threads change them: when a system call changes one,
  the other threads of the cell waiting on descriptors look again. (Before
  this change, such a thread only woke when some socket somewhere changed.)
- **A cell new to the scheduler** (forked, resumed, a template's child,
  woken by the host) makes its waiting calls again, joining the queues.
- **A wake from the host is kept** when the cell is not asleep yet: its
  next sleep ends at once. Without this, a cell preempted between its
  last output and its sleep missed the wake and slept on, which is how
  `a_woken_cell_runs_at_once` hung under load.
- **Latencies of the load only**: the server workload's latencies leave
  out cells queued before the load started.
- **Metric**: `density.wasted_wakeups_pct`, the share of wake-ups by a
  socket change after which the cell's first system call waited on
  descriptors again, from the server workload; with the existing
  `density.wake_*` and `density.sched_*` latencies.

### Out of scope

Exclusive waits (`accept` waking one waiter; the forked servers of the
workload wait in `accept` on their shared listener only at their start),
event masks per waiter (a reader is woken by a change that only made room
for writers of the same socket only when the queue was full), the global
sockets lock (B.2, a separate change), and waking a running cell's thread
that waits on descriptors before the cell is given back (as before).

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `scheduling`: a waiting cell runs again only when a socket it waits on
  changes, or a thread of its own changes a pipe, event counter or `epoll`
  instance, or at its deadline.
- `networking`: wait queues per socket.
- `benchmarks`: the wasted wake-up metric.

## Impact

`crates/rhizo-net` (wait queues, `end_call`, `take_woken`, `forget`),
`crates/rhizo-sched` (keyed waiting set: `wait(key, …)`, `wake`; the
wake-all is gone), `crates/rhizo-fs` (`Tables::changes`), `crates/rhizo-abi`
(waking the cell's own waiting threads), `kernel/src/cell.rs`,
`kernel/src/prof.rs`, `kernel/src/workloads.rs`, `kernel/src/tests.rs`,
`xtask/src/workloads.rs`, `bench/README.md`, `bench/baseline.json`.
