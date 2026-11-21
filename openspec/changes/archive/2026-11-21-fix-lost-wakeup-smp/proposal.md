# Proposal

## Why

The server workload (`density.*`, change `add-socket-wait-queues`) hangs
about once in 15 boots on 4 vCPUs, on main as well: every CPU halts while
cells still wait. CI runs it on 1 vCPU only, so nobody saw it.

Reproduced on QEMU TCG with 4 vCPUs (x86_64: 4 hangs or hour-long stalls
in about 32 boots), with the scheduler's state dumped by the boot CPU once
it had waited 2 s:

```
idle=0b1111 running=0 queues=[0, 0, 0, 0] sleeping=4000
next_wake=3600.21 s timer=(CPU 2, 3600.21 s) waiting=0 woken=false
```

Nothing is lost inside the scheduler: no run queue holds a cell, no socket
change waits to be drained, and an idle CPU's timer does cover the
earliest deadline. Two faults combine:

1. **The host's time limit wakes nobody.** The workload's host loop runs
   cells with `host.run(|_| now_ns() >= until)`. The boot CPU looks at that
   condition only when something wakes it. On one CPU it holds every
   cell's deadline itself, so it looks at least at each deadline. On
   several, another CPU's timer may hold the earliest deadline (design D1
   of `add-tickless-idle`) and the boot CPU waits for a wake-up interrupt
   only: when the cells next wake in an hour, the run outlives its limit by
   an hour. The same holds for every run stopped by time
   (`agent_figures`, `wire_rtt`).
2. **A template frozen on its way to its sleep.** The workload freezes each
   `worker` template once it has printed `ready`, but the template is
   asleep only after its next call, an hour's sleep. A slice that ends
   between the two (more likely under emulation, where new code is slow to
   translate) freezes a runnable template; its forks find no sleep to
   expire and all start the hour's sleep. Here the 2,000 `serve` and
   2,000 `ping` workers slept, and with nothing else due the run never
   reached its limit (1 above).

## What Changes

- `Host::run_until(at, stop)`: a run that ends at monotonic time `at` at
  the latest. The boot CPU never waits past `at`, whether it waits for its
  own timer or for a wake-up. The workload's host loop, `agent_figures`
  and `wire_rtt` use it instead of reading the clock in their stop
  condition.
- The server workload freezes a template only once it sleeps
  (`Host::asleep`, the safe point of the spec `workloads`).
- `cargo xtask bench --workload server --cpus N` runs the server VM with
  `N` virtual CPUs (1 by default, as CI and the baselines), for local runs
  only: it is refused with `--check` and `--update-baseline`.
- In-kernel test `a_run_with_a_time_limit_ends_on_time`, run on 1 and 4
  vCPUs: cells sleeping an hour, 20 runs limited to 50 ms each. Without
  the fix it hangs on 4 vCPUs (`ktest-smp` times out).

### Out of scope

- A 4-vCPU server run in CI: under TCG the 10,000 workers on 4 vCPUs make
  almost no progress in the load's 3 seconds (every system call of forked
  cells takes `SHARING`, a spinlock emulated vCPUs fight over), so it
  would take minutes and measure little; the in-kernel test covers the
  hang on every `ktest-smp` run.
- Waking more than one idle CPU when many cells become runnable at once
  (a wake goes to the lowest idle CPU, often the boot CPU): a latency
  matter, not a lost wake-up.

## Capabilities

- `scheduling`: a run with a time limit ends on time on any number of CPUs.
- `benchmarks`: server templates are frozen asleep; `--cpus` for local
  runs.

## Impact

- `kernel/src/cell.rs` (`Host::run_until`, the boot CPU's wait),
  `kernel/src/workloads.rs` (server loop and templates),
  `kernel/src/tests.rs` (new test), `xtask` (`--cpus`).
- The boot CPU's wait gets one more bound, only in runs with a time limit:
  no cell path of the `cell.*` figures uses one, and the `-icount` gates
  pass. The server workload's host loop now keeps its period on several
  CPUs; on 1 vCPU (CI and the baselines) its figures stay within noise.
