# Proposal

## Why

`add-smp-boot` brought the other CPUs online, but they only idle: every
cell still runs on the boot CPU. A machine with several cores runs as many
agents at a time as it has cores only when the kernel schedules cells on
all of them. This is the second change of milestone H17 (multi-core).

## What Changes

- **Cells on every CPU**: `Host::run` hands the host's cells to every CPU
  online. Each CPU takes a cell, runs it for a slice (preempted by its own
  tick), and gives it back; a cell runs on one CPU at a time, and its
  threads keep sharing that cell's slice (design D2 of `add-threads`).
  `Host::run` returns once every CPU has given back its cell, so every
  other host operation (spawn, fork, suspend, resume, freeze, kill) sees
  every cell, and all of its threads, stopped.
- **Per-CPU run queues** (`rhizo_sched::Scheduler`, still a pure,
  host-tested data structure): a CPU takes from its own queue first and
  steals the entry that has waited longest elsewhere when its own is empty;
  a cell goes back to its CPU's queue unless that queue is already longer
  than another's (then it moves); new cells join the shortest queue.
  `Host::step` keeps stepping through every cell on the boot CPU alone, in
  the order they have waited.
- **Locking**: one spinlock per host around the queues, the tag pool and
  the ended cells, held only to take or give back a cell; a global lock
  around copy-on-write table changes, taken only by cells that share memory
  with a fork.
- **Idle CPUs and wake-ups**: a CPU with nothing to run waits for the
  earliest sleeper or a wake-up; a CPU giving back a cell wakes an idle one
  when there is more work than it takes itself, and wakes the boot CPU when
  a cell ends (its stop condition may hold).
- **TLB coherence and per-CPU state**: AArch64 already broadcasts every
  cell TLB invalidation. On x86_64 (`INVLPG` is local) each cell records the
  CPUs whose TLB holds nothing stale for it; a CPU not in that set flushes
  the cell's PCID when it next enters it, and the set shrinks to the
  flushing CPU when the cell's translations shrink. A CPU lets go of a
  cell's tables and FP/SIMD state when it gives the cell back. Per-CPU:
  the loaded root, FP trap state, FS base, FP owner.
- **Tests**: host property tests of the queues under any interleaving of
  CPUs (nothing lost or run twice, no idle CPU while work waits, nobody
  starves); in-kernel tests on 4 vCPUs: CPU-bound cells use every CPU,
  spinning cells on every CPU do not starve another cell, twenty programs
  of five kinds at once stay isolated and return all memory, and a cell
  suspended between parallel runs continues as if never stopped. The whole
  in-kernel suite already runs on 4 vCPUs (`ktest-smp`), now with cells on
  every CPU.
- **Benchmarks** (a `bench-cell` boot with 4 vCPUs per VMM):
  `cell.parallel_us`, `cell.scaling_pct`, `cell.ipi_wake_us`.

### Out of scope

Threads of one cell running on several CPUs at once (and the TLB
shootdowns that needs on x86_64): a cell stays the unit a CPU runs, as in
`add-threads`; that is the next change. NUMA, CPU affinity requests,
priorities, and stopping a single cell's threads while the others run.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `multi-core` (added by `add-smp-boot`): cells isolated on every CPU, TLB
  coherence, cells stopped before host operations, shared copy-on-write
  memory changed one CPU at a time.
- `scheduling`: per-CPU queues, stealing, balancing; stepping on one CPU.
- `benchmarks`: multi-core metrics.

## Impact

`crates/rhizo-sched` (per-CPU queues), `kernel/src/cell.rs` (shared
state, run on every CPU, per-CPU FP owner, TLB sets, sharing lock),
`kernel/src/arch/*/{user,irq,smp}.rs` (per-CPU state, tick start/stop,
`leave_user`), `kernel/src/smp.rs`, `kernel/src/sync.rs`,
`crates/rhizo-core/src/sync.rs` (`get_mut`), `cells/src/bin/spin.rs`
(optional round count), `kernel/src/tests.rs`, `xtask/src/bench.rs`,
`bench/README.md`, `AGENTS.md`, `docs/security.md`.
