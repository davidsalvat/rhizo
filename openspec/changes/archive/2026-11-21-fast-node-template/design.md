# Design

## Where the warm path's time went

The Node.js benchmark kernel built with `profile`, its warm forks
wrapped in a breakdown (a local measurement, not committed), on QEMU
`microvm` under `-icount shift=0,sleep=off` (one nanosecond per
instruction; an idle wait still advances the clock to its deadline).
Seven forks, per fork:

| | Frozen at `warm` (before) | Frozen in the pause, before D1 | Frozen in the pause, after D1 |
|---|---|---|---|
| `workload.node_warm_us` | 27.9 M | 79.5 M | 29.6 M |
| `idle` | 0 | 50.0 M | 0 |
| `user` | | 23.6 M | 23.6 M |
| `syscall` | | 1.5 M | 1.5 M |
| `copy` + `frames` + `cow` | | 3.9 M | 4.0 M |
| copy-on-write faults | | 938 | 936 |
| `teardown` | | 0.27 M | 0.28 M |

`workload.node_cold_us` is 250 M in every run. With the template frozen
in its pause, every fork was idle for exactly the script's 50 ms: on CI
with KVM, 57.5 ms warm is those 50 ms and about 7.5 ms of work. The rest
is small. Copy-on-write over V8's heap is about 940 faults (3.7 MiB) per
fork, 13 % of the fork. Forks share page tables and copy no page
eagerly. The Node.js VM has one CPU, so there are no shootdowns or
inter-processor interrupts. Futex waits without a timeout behave as
specified. Under TCG without `-icount` (real clock) the breakdown has the
same shape (`idle` 0 when frozen at `warm`).

Two things together:

1. **The kernel restarted the timeout.** A call that waits on descriptors
   (design D3 of `add-sockets-and-poll`) puts its thread back at the
   system-call instruction, in `Wait::Io { until }`, and keeps its
   deadline in the live cell's record (`Live::deadlines`) for when the
   call is made again. `freeze` keeps the `Cell` and its files, not that
   record, so a template has no deadlines. `spawn_from_template` makes the
   child's threads runnable (`Threads::expire_timers`). The main thread
   then makes `epoll_pwait(…, 50)` again with no deadline kept, and waits
   50 ms more. `Host::wake` had the opposite flaw: the deadline was still
   kept, in the future, so the call made again waited until it.
2. **The benchmark froze the template at a moment that depends on the
   VMM.** It froze the template as soon as `warm` was printed, after the
   step whose slice printed it. With KVM, that slice runs on until the
   main thread blocks in `epoll_pwait`. Under `-icount` or TCG it ended
   earlier, before the timer was armed, and the fork then computed its
   timeout from the clock and did not wait. So the instruction counts
   (`-icount`) and the CI timings measured different things.

The 50 ms busy wait after `freeze` in the benchmark is fair, and stays.
libuv compares its timer's due time with the clock (`uv__io_poll`
subtracts the time spent from the timeout and waits for the rest). So a
fork made less than 50 ms after the template's `epoll_pwait` began would
wait the rest even with D1, as a program should: the timer is the
program's own. Measured without the busy wait, the first fork waited
49 ms and the next six none. `spawn_from_template` waits for nothing but
the fork. `Host::run` stops when the forked cell has ended.

## D1. Expiring a live cell's timers covers waits on descriptors

`Live::expire_timers` replaces the three calls of
`Threads::expire_timers` on a live cell (spawn from a template,
`Host::wake`, a host wake kept for the next wait). It sets every kept
deadline to 0, gives each thread in `Wait::Io { until: Some(_) }` a kept
deadline of 0 if it has none (a template's), then makes the threads
runnable. The call made again finds its deadline passed (`block` returns
`None`) and returns what is ready, or nothing: `epoll_pwait` returns 0,
as when its timeout runs out.

Alternatives:

- **Keep the deadline in the thread** (`rhizo-cell`), so that freeze,
  fork and snapshots carry it. This is the more general fix, but it
  changes the snapshot format, which open change `add-shared-snapshots`
  also changes, and templates expire deadlines anyway.
- **Keep `Live::deadlines` in the template.** The fork would get the old
  deadline, which is already past by the time the fork runs on CI but not
  in general, and still not expired as the spec requires.
- **Fix the benchmark only** (freeze before the main thread blocks). The
  bug would stay for every program whose template waits in `poll` or
  `epoll` with a timeout, which is how event loops wait.

## D2. The template is frozen in its pause

`Host::waits(id)` says whether none of a live cell's threads can run.
The benchmark steps the template until it has printed `warm` and waits,
then freezes it. Every VMM then freezes the same state: the main thread
in `epoll_pwait` with the timer's timeout, the others on their condition
variables. `Host::asleep` does not fit: it is false while a thread waits
on descriptors, by design D3 of `add-agent-workload`.

## D3. Tests

`a_wait_on_descriptors_keeps_no_time` (in-kernel, both architectures)
runs the `sleeper` cell with `epoll`. It creates an empty `epoll`
instance, prints `ready` and waits an hour in `epoll_pwait`, then prints
`timed out`. A template frozen there is forked. Another copy, live, is
woken by the host. Within four slices both have printed `timed out` and
exited with 0. Before D1, each hung for its hour (the test step timed
out). The Node.js benchmark is the end-to-end check:
`workload.node_warm_us` drops by 50 ms on CI.

## Metrics

| | before | after |
|---|---|---|
| `workload.node_warm_us`, KVM (CI) | 57.4 ms | about 7.5 ms expected (CI figures in the pull request) |
| `workload.node_warm_us`, `-icount`, template frozen in its pause | 79.5 M | 29.6 M |
| `workload.node_cold_us`, `-icount` | 250 M | 250 M |

CPython, numpy and the agent fork are not affected. Their templates and
forks are made while their threads sleep (`select` without descriptors,
`nanosleep`) or run, not while they wait on descriptors with a timeout,
so no other benchmarked path changes.
