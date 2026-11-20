# Proposal

## Why

The thesis promises thousands of mostly idle cells per node at a few KB of
overhead each, but nothing measures it, and nothing protects the kernel
when cells together fill memory: cell pages come from the same frames as
the kernel heap, and a heap that cannot grow panics. One cell is bounded
(64 MiB of files) but many are not. A substrate for untrusted agents must
refuse the cell, never crash.

## What Changes

- A kernel reserve: frames for cells (pages, page tables, file blocks,
  snapshots) are handed out only while the reserve stays free for the
  kernel heap. Running out is the cell's error (`ENOMEM` from `mmap`/`brk`,
  `ENOSPC` from a file write, a fault on a copy-on-write page it cannot
  get); for the host, `spawn`, `fork`, `suspend` and eager `resume` fail
  and leave everything as it was.
- A test cell that idles (`idle`), and an in-kernel test with 1000 idle
  cells forked from one template: all live at once, all memory returned.
- In-kernel tests that flood memory with cells (memory and files) and check
  the kernel keeps running and gets every frame back.
- Benchmarks: `cell.idle_bytes` (memory per idle live cell, from 1000
  forks of a template) and `cell.suspended_bytes` (snapshot of an idle cell).

### Out of scope

Per-cell quotas set by the host, swapping snapshots to disk, multicore.

## Capabilities

### Modified Capabilities

- `cells`: memory pressure is the cell's error, never a kernel panic;
  density.
- `benchmarks`: idle and suspended cell size.

## Impact

`kernel/src/cell.rs` (reserve, checks), `crates/rhizo-abi` (`Services`
check before file blocks), `cells/src/bin/idle.rs`, `kernel/src/tests.rs`,
`xtask/src/bench.rs`, docs.
