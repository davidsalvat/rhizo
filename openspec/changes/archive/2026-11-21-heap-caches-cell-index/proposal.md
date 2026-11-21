# Proposal

## Why

PoC line F.4, low-level optimization, looked at two suspects: the kernel
heap (power-of-two size classes behind one global lock, up to 50 %
internal waste) and the lookup of a cell by its identifier in the
scheduler, linear in the number of cells (design D4 of
`host-ops-per-cell`). A profile at the server workload's 10,000 cells
(design D0) found:

- **The cell lookups cost a third of every host operation.** Each one
  took 29,000 instructions on average, over the sleep list's 6,000
  workers: 26 of the 78 million instructions the host's 900 operations
  took during the load.
- **One kernel record per cell wastes 690 bytes.** At 10,000 cells the
  heap holds 10,000 blocks of 1345 to 1360 bytes, each in a 2048-byte
  block: 6.9 MB, 2.5 % of the memory a worker costs
  (`density.cell_bytes`).
- **Per-CPU caches would buy nothing measurable.** Forking from a
  template allocates one block, a spawn two, a system call none; on 4
  CPUs the heap's lock made a CPU wait for well under 0.1 % of its time.

## What Changes

- The heap gets eight size classes between the powers of two (48, 96,
  192, 336, 448, 672, 816 and 1360 bytes: each the largest multiple of 16
  bytes of which a page holds a whole number of blocks), for requests
  aligned to 16 bytes or less. The power-of-two classes keep their
  indices and serve the larger alignments (design D1).
- The scheduler keeps a hint per sleeper: the wake time of the last entry
  put to sleep, at the slot of its key's low bits. A lookup by key binary
  searches the sleep list for that time and checks the key; a stale hint
  falls back to the search. Only `sleep` writes it: taking a cell to run
  and giving it back do no more work (design D2).
- The server workload times its kills too, and reports the median
  suspend, fork and kill as `density.suspend_us`, `density.fork_us` and
  `density.kill_us` (design D3).

### Out of scope

Per-CPU caches (magazines) in front of the heap's free lists: the profile
shows nothing for them to win (design D0). Runnable cells are still
found by comparing keys over the run queues, which stay short unless
cells are CPU bound; indexing them would cost a store on every switch.

## Capabilities

### Modified Capabilities

- `kernel-heap`: classes between the powers of two.
- `scheduling`: sleepers found without a search.
- `benchmarks`: host-operation latencies at the server's target.

## Impact

`crates/rhizo-mm/src/heap.rs`, `crates/rhizo-sched/src/lib.rs`,
`kernel/src/workloads.rs`, `kernel/src/tests.rs`,
`xtask/src/workloads.rs`, `bench/README.md`. No change to the switch path
(`cell.switch_insns`) or to any `-icount` figure beyond noise.
