# Proposal

## Why

A host operation on one cell (suspend, resume, fork, freeze, kill, spawn
from a template) needs every CPU out of cell code today: the caller ends
`Host::run`, which stops every secondary CPU and waits for each to give
back its cell; the operation runs; the next `run` starts them all again
(spec `multi-core`, "Cells stopped before host operations"; designs of
`add-heap-trim` and `add-parallel-threads`). With thousands of busy cells,
every snapshot or fork pauses the whole machine, and part of the server
workload's remaining tail at 10,000 cells is those operations every
10 ms (design D6 of `add-socket-wait-queues`). This is PoC line F.3,
scale.

## What Changes

- `Host::run`'s stop condition gets `&mut Host`: every host operation may
  be made from there, on the boot CPU, while the other CPUs go on running
  their cells. Between runs nothing changes.
- An operation on cell X takes X out of the scheduler under the host's
  lock. If CPUs run X, only those are interrupted; they give X back at once
  (`CLAIM`, checked at interrupts and before entering a gang's code) and the
  host waits for it. A cell nobody runs is taken at once. Cells other than
  X are never stopped.
- The state an operation shares with running cells (frame reference counts,
  copy-on-write tables, page and image caches, heap, sockets, tags, TLBs)
  is made safe with the locks it already has; fork needs none (design D3).
- The scheduler keeps each entry's key next to it: finding a cell by
  identifier compares keys instead of reading every cell (design D4).
- The server workload makes its host operations from the stop condition.
- A new test program `progress` counts where the host can read it without
  stopping it (`Host::watch`, tests and benchmarks only).
- New metrics `cell.host_op_stall_us` and, for comparison,
  `cell.host_op_stall_stopped_us` (4-vCPU `bench-cell` boot).

### Out of scope

The end of a run still stops every CPU (a caller that wants every cell
quiet asks for it). The control kernel keeps answering requests between
runs (design D7). Freeing the heap window's retired page tables still
waits for a moment when every secondary CPU idles.

## Capabilities

### Modified Capabilities

- `multi-core`: host operations stop only the CPUs running their cell.
- `scheduling`: finding a cell by identifier, and filing back cells a host
  operation took.
- `benchmarks`: the host-operation stall metrics.

## Impact

`crates/rhizo-sched` (`Keyed`, keyed `remove`/`find`, `pop`),
`kernel/src/cell.rs` (`Host`, `take`, `give_back`, gangs),
`kernel/src/workloads.rs`, `kernel/src/tests.rs`,
`cells/src/bin/progress.rs`, `xtask/src/bench.rs`, `bench/README.md`.
Benchmarked paths gain one compare and two stores per slice.
