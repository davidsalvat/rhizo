# Design

## D1. Laptop: agents opened from one template, mostly suspended

`agent ADDR PORT` warms up (fills a 1 MiB history, digests
`/shared/notes.txt`), prints `ready` and sleeps; the kernel freezes it as a
template. Each fork starts at once (templates keep no time), opens its
session (writes `/work/history.txt`, prints `open`) and sleeps until its
user writes. 50 agents are opened, 45 suspended, 5 stay live.

- **User messages.** With no vsock, the host stands in for the user with
  `Host::wake(id)`: it ends the cell's sleeps and times out its timed waits
  (`Threads::expire_timers`, as a template's fork does) and files it again.
  A woken agent runs one turn: an HTTP request to the model over TCP
  through the network card, the answer appended to its history, and a tool
  action (the shared notes read back and checked), then prints `turn`.
  `workload.agent_turn_us` is the time from the wake to `turn` (the name
  the README roadmap uses); `laptop.resume_turn_us` the same right after a
  lazy resume. Lost: a real message's data; nothing reads it yet.
- **Model.** A thread of `xtask` per address answers any HTTP request with
  a fixed completion after `--llm-delay-ms` (0 by default, so the turn
  measures Rhizo, not a sleep), on the host side of the guest's network
  (`llm=ADDR:PORT`, next to the echo server).
- **Shared folder.** No virtio-fs or vsock: the folder is a file seeded
  into the template's own files at boot, shared copy-on-write by every
  fork. It is shared between agents, not with the host.
- **Idle.** Over a window (2 s; 60 s in the soak) the 5 live agents sleep,
  45 are suspended, and a `sleeper` cell ends the window, so the CPUs idle
  the way the scheduler makes them idle (`Host::run`). The guest counts
  every return from waiting for an interrupt (`laptop.idle_wakeups_per_s`);
  the host reads the VMM's CPU time (D6). The VM has 2 vCPUs: a laptop
  gives its agents more than one, and every CPU's idling counts.

Alternatives: a real model server (needs a model, measures the model);
driving turns from the agents' own timers (the turn's start is then not
the host's, and timers would wake the idle window).

## D2. Server: 10,000 workers in a fixed mix, constant-rate host operations

`worker MODE PERIOD_MS SECS [PORT]` prints `ready` and sleeps (frozen
there); from its first wake it waits a random part of a period (so forks
spread), then runs one operation per period for `SECS` seconds and prints
`ops=N`. Modes: `cpu` (20,000 xorshift rounds), `file` (4 KiB written and
read back), `serve` (listens before `ready`, so its forks share the
listener; accepts one connection and echoes until it closes) and `ping`
(connects, one 64-byte round trip per operation). Groups of ten forks
(4 `cpu`, 2 `file`, 2 `serve`, 2 `ping`) are made until the target
(10,000, `cells=`) or until memory keeps less than 32 MiB of headroom
(`limit=memory`, or `limit=spawn-refused`): `density.cells` and the reason.

The workers run 3 s (600 s in the soak) at 200 ms per operation: an
offered load of 50,000 operations per second at 10,000 cells. Every 10 ms
the host suspends and lazily resumes one `cpu` or `file` worker (sockets
do not survive a suspend yet) and forks one, killing the previous child:
`Host` operations happen between runs, which with several CPUs would stop
them all, so the workload uses one vCPU. `density.ops_per_s` counts the
operations of workers that exited with 0 over the load's wall time; any
other ending fails the run. `density.cell_bytes` is the most memory in
use during the load over the cells.

Alternatives: a closed loop (every cell always runnable) measures a
CPU-bound queue, not agents that mostly wait; spawning every worker from
its executable costs a copy of the image per cell instead of a shared
template.

## D3. Out of memory, pushed on purpose

After the load (`fill_memory`, also an in-kernel test): fork idle cells,
each stepped into its sleep, until the kernel refuses one
(`density.oom_cells`); then spawning from an executable must fail with
`OutOfMemory`, forking a live cell and an eager resume must be refused;
with a quarter of the cells killed, `hog` must get `ENOMEM`; once every
cell ended, free frames plus heap pages and the heap's live allocations
must be what they were. The xtask side fails the benchmark if any of this
does not hold; a kernel panic fails the run anyway.

## D4. Latencies: histograms, recorded only while measured

`rhizo_core::stats::Histogram`: log-linear buckets (16 per power of two,
976 counters for any `u64`), constant-time record, quantiles within 1/16
above the exact value, host-tested against exact quantiles. The kernel
(`prof`) keeps two in one lock (scheduling, wake-up, in nanoseconds) and
one per CPU for system calls (counter cycles, converted when reported).
Each live cell carries two stamps:

- **event**: its deadline when it sleeps or waits with one, or the time of
  the last socket change made by a system call when the waiting set is
  woken (`wake_waiting`);
- **queued**: when it joined a run queue: preempted, yielding, new, or
  made due. Sleepers become due inside `Scheduler::next`, so `take` first
  calls the new `Scheduler::wake_due(cpu, now, f)` (what `next` does, with
  a callback per entry) while measuring.

When `take` hands a cell to a CPU: scheduling latency = now − queued, and
wake-up latency = now − event when it had one. System-call latency is the
handler's time, locks included. Off, the hooks cost a relaxed load per
system call and per filing; no clock is read. Alternatives: stamping
inside `rhizo-sched` (it never reads a clock; its callers do), or keeping
samples (10,000 cells × 15 operations per second is too much memory).

## D5. Where the time goes, with counters meant to be replaced

Until F.1's breakdown lands: cycles idle (waiting for an interrupt),
in system calls (the histogram's sum), in scheduling decisions (`take`
and `give_back`), in host operations (timed by the workload); counts of
runs, socket changes, cells woken by them (the whole waiting set wakes on
any change), copy-on-write and lazy faults, interrupts taken in cell code.
The server workload prints them in a `rhizo-server-breakdown` line that
`xtask` shows; the rest of the wall time is cell code, faults and
switches.

## D6. The host's view of idle

The guest prints `rhizo-mark NAME` lines; `vmm::run` stamps each when it
reads it, with the VMM process's CPU time (the first field of every
`/proc/PID/task/*/schedstat`, nanoseconds, all threads). Between
`idle-start` and `idle-end`, CPU time over wall time is
`laptop.idle_host_cpu_pct`. Alternatives: `/proc/PID/stat` (10 ms ticks:
too coarse for a 2 s window), `getrusage` of children (only after exit).

## D7. In CI, and weekly

`cargo xtask bench` runs both workloads after the cell figures, on every
VMM (the laptop where a network card exists; otherwise skipped with the
reason): CI runs them on every pull request. `--workload laptop|server`
runs one alone, `--soak` the long versions, `--cells` and
`--llm-delay-ms` change the load. The weekly workflow runs
`bench --workload server --soak` and `--workload laptop --soak` on
Firecracker and publishes the results like the benchmark job.

## D8. The gate

`density.cells`, `density.ops_per_s` and `density.oom_cells` are better
when higher: the gate fails them when the baseline is more than the
threshold above the value (by ratio, so +100 % allows halving). Metrics
enter the `ci` baseline once two CI runs agree within the threshold; the
tails (p99.9) and the host CPU stay ungated on shared runners, listed in
`ungated.ci` with the reason in `bench/README.md`.
