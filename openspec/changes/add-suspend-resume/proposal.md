# Proposal

## Why

The thesis of Rhizo is that suspending and resuming an agent cell should be a
cheap native kernel operation, not a VM snapshot taken from outside. Agents
spend most of their life waiting; a host should be able to put an idle cell
away as plain bytes and bring it back in microseconds. H4 makes suspend and
resume real, proves that a resumed cell is indistinguishable from one that
never stopped, and restores lazily: a resumed cell starts running at once and
its pages come back when it first touches them, an in-kernel analogue of
Firecracker restoring with `userfaultfd`.

## What Changes

- New crate `rhizo-snapshot`: a versioned, architecture-tagged binary format
  for a suspended cell (registers, program break, mapping cursor, pending
  sleep, and every mapped page with its permissions and contents), encoded
  explicitly in little endian, with host tests (round trip, corruption and
  version checks).
- `rhizo-mm`: a walk over the user half of a page table (each leaf page with
  its permissions).
- `rhizo-cell`: a pager hook, so that system calls touching a page not yet
  restored fault it in instead of returning `EFAULT`.
- Kernel: `Host::suspend(id)` captures a cell into a snapshot and frees all
  its memory; `Host::resume(snapshot, mode)` makes it runnable again, eagerly
  (all pages mapped before it runs) or lazily (pages mapped on first touch,
  from the cell or from a system call).
- Snapshot-equivalence tests: a cell suspended and resumed at many points,
  eagerly and lazily, prints exactly what an uninterrupted run prints.
- Benchmarks: `cell.suspend_us`, `cell.resume_us` (lazy: until the cell can
  run), `cell.resume_eager_us`, `cell.resume_touch_us` (lazy resume plus
  touching every page), `cell.snapshot_bytes`, for a cell with 8 MiB of
  touched memory.

### Out of scope

Writing snapshots to disk or the network (the format is ready for it; there
is no storage driver), resuming the same snapshot twice at once (that is a
fork, H5), compression and deduplication, open files beyond the console,
floating-point state (cells have none until H6).

## Capabilities

### New Capabilities

- `snapshots`: suspend, resume, the snapshot format, equivalence.

### Modified Capabilities

- `cells`: a cell can be suspended and resumed.
- `benchmarks`: suspend/resume metrics.

## Impact

- New crate `rhizo-snapshot` (depends on `rhizo-cell` for the cell state).
- Kernel `cell.rs`: suspend, resume, lazy paging on faults.
- New test cells and benchmark lines.
