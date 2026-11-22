# Proposal

## Why

`cargo xtask --arch x86_64 test --only ktest-node` (2 CPUs) failed twice
in unrelated pull requests, on Firecracker (#146) and QEMU (#138), with
`PANIC at kernel/src/tests/cells.rs:127:28: cell 7 has not ended`, and
passed on rerun.

That message hid the real error. `node.rs` reported an error of
`Host::run_to_pause` with `ended(host, id)`, which panics itself when the
cell is still live. The console shows what happened: cell 7, the template
of `a_node_cell_suspends_resumes_and_forks`, printed `warm` and then its
answer 61 ms later. The hidden error was `cell 7 printed past its pause`.

The cause is in `run_to_pause` (change `fix-warm-wait-race`), when the
template is multi-threaded and other cells are runnable. It steps the
host round robin. After `warm`, Node.js's main thread arms its 50 ms timer
and blocks while a V8 worker it woke can still run. The held slice ends
at that block, and the cell waits behind the other runnable cells. Here
those were the resumed cell and its two forks, each answering in full
10 ms slices. After a round or two the timer is over, and the main thread
prints its answer before the host ever sees every thread wait. A
single-threaded template has no such window: its block files it asleep,
and the host sees that at once.

## What Changes

- **Diagnostics.** `run_to_pause`'s error includes the output so far and,
  for a cell that ended, its ending. Its callers report that error alone,
  with no helper that can panic over it (`node.rs`, `fork.rs`,
  `packages.rs`).
- **`run_to_pause` runs the held cell first once its line is out.**
  While the cell has printed its line and nothing after it, each step runs
  that cell's next slice if it can run, before any other cell. It reaches
  its pause in its own CPU time, as it does alone, however many cells are
  runnable. Steps fall back to round robin while it waits for anything
  else.
- **`Scheduler::take(key)`** in `rhizo-sched`: removes an entry only if it
  is runnable.

### Out of scope

- Changing the pause of Node.js's script, or any program: they are also
  the Linux reference, and a longer timer would only make the race rarer.
- Scheduling outside `run_to_pause`.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workloads`: a template is frozen at its pause even when other cells
  are runnable and another of its threads can still run as the pause
  starts.

## Impact

- `kernel/src/cell/pause.rs` (`run_to_pause`, `step_held`);
  `Host::collect` becomes `pub(super)` (`kernel/src/cell/host.rs`).
- `crates/rhizo-sched/src/lib.rs`: `Scheduler::take`, host tests.
- `cells/src/bin/threads.rs`: mode `pause`; a new in-kernel test in
  `kernel/src/tests/threads.rs`.
- Callers in `kernel/src/tests/{node,fork,packages}.rs`.

Benchmarks freeze their templates with nothing else runnable, so
`step_held` runs the slice `step` would have run: their figures do not
move.
