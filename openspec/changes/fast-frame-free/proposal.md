# Proposal

## Why

Wave F.4 asked whether the frame allocator should get per-CPU frame
caches and a pool of frames zeroed by idle CPUs. We profiled it first
(design D1). The profile does not justify either one:

- **Lock contention is negligible.** In the 4-vCPU server workload (10,000
  cells, about 97,000 operations), CPUs waited for the frames lock 127
  times in all, against 1,982 waits for the heap lock.
- **The single-frame path is already cheap.** Under `-icount` on x86_64,
  a cell's `alloc` costs about 60 instructions and its `free` about 75,
  locking included. That is 23 of each per `hello` run, or about 3.5 % of
  `cell.run_insns`. Per-CPU magazines would save about half of it and
  would add balance accounting, draining on shortage, and frames held
  away from 2 MiB allocations.
- **Zeroing is costly but happens at the wrong time for a pool.** It
  takes about 540 instructions per page, 23 pages per `hello` run (14 %
  of `cell.run_insns`). The benchmarks run back to back with no idle
  time, though, so a pool that idle CPUs fill would be empty in every
  gated figure. It would help only the first spawn after an idle period,
  and it would need a "zeroed frame" path through `paging`,
  `rhizo-cell` and `rhizo-abi`.

The profile found a real waste elsewhere. **Freeing a run of frames costs
about 9 instructions per frame**, because the double-free check tests one
bit at a time. Freeing a 2 MiB page costs about 4,600 instructions,
against about 80 to allocate it, and every large page goes through this:
copy-on-write of large pages, teardown, suspend, eager resume.

## What Changes

- **`FrameAllocator::free_contiguous` checks and frees a run a word at a
  time** (64 frames per step). It still panics on a double free and names
  the lowest frame of the run that is already free, as before.
- **Freeing one frame takes a one-bit path**, so the common case gets
  cheaper too, not more expensive.
- No new metric. The existing instruction counts show the change:
  `cell.fork_touch_insns`, `cell.resume_eager_insns`,
  `cell.resume_touch_insns`, `cell.suspend_insns` and `cell.run_insns`.

### Out of scope

- Per-CPU frame caches and an idle-zeroed pool (see Why). They can be
  revisited if a multi-CPU workload shows contention on the frames lock,
  or if the benchmarks gain a figure measured after an idle period.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `physical-memory`: freeing a run of frames costs per bitmap word, not
  per frame.

## Impact

`crates/rhizo-mm/src/frames.rs` only. No API change. No baseline change:
every instruction count stays the same or goes down.
