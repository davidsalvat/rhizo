# Proposal

## Why

`add-smp-scheduling` runs cells on every CPU, but each cell on one CPU at a
time: the threads of a cell take turns on that CPU (its design D1 lists
this as lost). An agent whose program uses threads (a thread pool, a
parallel computation, an interpreter's workers) gets one core however many
are idle. This change lets the threads of one cell run on several CPUs at
once, without giving up what makes a cell a cell: the whole cell still
stops before it is suspended, forked or frozen.

## What Changes

- **A cell may run on several CPUs at once.** The run queues still hold
  cells (fairness between cells, whole-cell host operations). A CPU that
  finds no cell in any queue joins a running cell that has a runnable
  thread no CPU runs, and runs that thread; a CPU leaves the cell when its
  thread blocks and the cell has no other thread for it, at the end of its
  slice, or when its thread yields. The last CPU to leave gives the cell
  back to the queues, so between runs every thread of every cell is
  stopped. A single-threaded cell, `Host::step` and a kernel with one CPU
  keep the single-CPU path unchanged.
- **One lock per cell running on several CPUs**, around its system calls,
  the faults that change its tables (copy-on-write, stack growth, lazy
  resume) and the choice of a thread. Each CPU keeps the registers of the
  thread it runs.
- **Wake-ups across CPUs**: a system call that makes a thread runnable
  (`futex` wake, `clone`, a thread exit that wakes a joiner) offers it to
  idle CPUs and wakes one with an inter-processor interrupt. The earliest
  deadline of the cell's waiting threads is offered too, so an idle CPU
  wakes for a futex wait that times out or a sleep that ends.
- **Ending on every CPU**: `exit_group`, the last `exit` or a fault ends
  the cell; the other CPUs running its threads are interrupted and leave
  it, and the last one frees it.
- **TLB shootdowns on x86_64**: when a system call or fault shrinks the
  translations of a cell (unmap, protection change, copy-on-write) while
  other CPUs run its code, those CPUs are interrupted and kept out of the
  cell's code until the change is complete, and drop the cell's PCID
  before they enter it again. AArch64 keeps its broadcast `TLBI` and sends
  no interrupt.
- **Scheduling unit in `rhizo-sched`**: a new pure structure, `Gangs`,
  records the cells running on several CPUs, which CPUs run each and what
  each offers to idle CPUs; host property tests over random interleavings.
  `rhizo-cell` gains choosing a thread by identifier while skipping
  threads that run elsewhere, and making a thread current for its system
  call.
- **Tests**: `parsum`, a static glibc program summing in parallel with
  mutexes and condition variables (checked against Linux), runs with 4
  threads on 4 vCPUs faster than with one; `shootdown`, a cell where one
  thread keeps reading a page that another unmaps, must fault and never
  read it after the unmap; a cell computing on several CPUs, suspended,
  resumed and forked between runs, gives the uninterrupted result.
- **Benchmarks** (the 4-vCPU `bench-cell` boot): `cell.thread_scaling_pct`
  (4 threads of one cell on 4 vCPUs against 1 thread) and
  `cell.shootdown_us` (one `munmap` of a page while the cell's other
  threads run on the other CPUs).

### Out of scope

System calls of different cells in parallel (every call still holds the
sockets lock, design D9 of `add-smp-scheduling`), per-thread CPU accounting
and priorities, CPU affinity, signals, and waking a running cell's thread
that waits on descriptors before the cell is filed (as before).

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `threads`: threads of a cell run in parallel; futex wake-ups across CPUs.
- `scheduling`: threads share their cell's time unless CPUs are idle; a
  cell on several CPUs.
- `multi-core`: TLB coherence with shootdowns; cells stopped before host
  operations whatever the number of CPUs their threads ran on.
- `workloads`: a parallel pthreads program.
- `benchmarks`: thread scaling and shootdown metrics.

## Impact

`crates/rhizo-sched` (`Gangs`), `crates/rhizo-cell/src/threads.rs`
(`pick`, `make_current`, `get`), `kernel/src/cell.rs` (gangs, the cell
lock, shootdowns), `kernel/src/sync.rs` (`get_mut` in every build),
`cells/c/parsum.c`, `cells/src/bin/shootdown.rs`, `cells/expected.rs`,
`kernel/src/tests.rs`, `xtask/src/bench.rs`, `bench/README.md`,
`AGENTS.md`, `README.md` (feature list).
