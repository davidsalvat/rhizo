# Proposal

## Why

Forking a whole agent environment, memory included, is the operation agent
workloads need most and today's sandboxes cannot do natively: they snapshot
a microVM and restore copies. Rhizo's thesis says a fork should cost about
as much as creating an empty address space. H5 adds `fork(n)` as a kernel
primitive with copy-on-write that shares not only the memory but the page
tables too, so a fork's cost does not grow with the size of the cell. It
also adds pre-warmed templates: cells prepared once and forked on demand.

## What Changes

- Frame reference counts, so a frame or a page table can belong to several
  cells; freeing drops a reference and returns the frame with the last one.
- Lazy copy-on-write with shared page tables, in `rhizo-mm` and host-tested:
  forking marks the parent's top-level entries read-only and copies them to
  the child; the first write under a shared table copies only the tables on
  the path to the written page, and then the page.
- Effective permissions: translation and user-memory access honor read-only
  table entries, so system calls can never write through a shared page; the
  pager resolves copy-on-write faults from system calls.
- Kernel: `Host::fork(id, n)` creates `n` children identical to the cell;
  write faults on shared pages are resolved transparently; templates
  (`Host::freeze`, `Host::spawn_from_template`).
- Tests: children and parent see their own writes, copy-on-write from system
  calls, equivalence of a forked child with its parent, frames all returned,
  forks of forks.
- Benchmarks: `cell.fork_us` and `cell.fork_pt_frames` for a 50 MiB cell,
  `cell.fork_touch_us` (fork plus writing every page),
  `cell.spawn_from_template_us` against `cell.spawn_us`.

### Out of scope

A Linux `fork`/`clone` system call (cells are forked by the host, not from
inside), sharing memory between live cells on purpose, memory accounting
per cell, multicore TLB shootdowns (one CPU).

## Capabilities

### New Capabilities

- `fork`: forking cells, templates.

### Modified Capabilities

- `virtual-memory`: reference-counted frames, shared tables, effective
  permissions.
- `kernel-testing`: the test VM time limit counts from the last output, so
  the growing suite does not hit it on slow (emulated) runners.
- `benchmarks`: fork and template metrics.

## Impact

`rhizo-mm` (refcounts, copy-on-write), `rhizo-cell` (space operations
unshare before changing a mapping), kernel `cell.rs`, both architectures'
fault decoding (write permission faults), new test cells and metrics.
