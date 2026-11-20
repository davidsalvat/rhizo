# Proposal

## Why

Every spawn loads its executable again: the loader allocates, zeroes and
copies every page of every segment, and on AArch64 the instruction cache is
then synchronized over the whole image. For CPython (7.4 MB of segments)
that is about 0.6 ms of a 17.6 ms cold start, paid again by every cell,
while a Linux process started from the same executable finds it in the page
cache and maps it. Rhizo already shares frames between cells with
reference counts (`rhizo-mm` `cow`); a spawn can start from a space the
executable was loaded into once, the way `spawn_from_template` starts from
a frozen cell.

## What Changes

- **Load once, start many**: `rhizo_cell::spawn` is split into `load` (a
  hierarchy holding only the segments) and `start` (kernel half, stack,
  first thread on a hierarchy holding the segments); `spawn` is the two in
  a row, so there is one loader.
- **A copy with its own tables**: `PageTable::clone_lower_half` copies a
  hierarchy's user half into new tables, shares every read-only page (one
  more reference) and gives the copy a private copy of every writable page.
  The source is only read.
- **Executables kept loaded**: the kernel keeps the spaces of the last four
  executables spawned (by the image's address and length; images are
  `'static`), never run. A spawn of a kept executable starts its cell on a
  copy of that space; a spawn of another loads it first. The instruction
  cache is synchronized once per kept executable, not per spawn.
- **Memory**: the kept spaces take frames as cells do (never the kernel's
  reserve) and belong to the kernel; when frames for cells run short, every
  kept executable is dropped before any cell is refused memory.
- **Tests**: host tests of the copy (also in the copy-on-write model check)
  and of a cell started on a copy against a directly spawned one; in-kernel
  tests that a kept executable starts fresh every time, gives way to cells
  under memory pressure, and that its cells suspend, resume and fork. The
  `same_addr` cell checks that its static starts with the executable's
  value. Tests that compare free memory before and after cells now do so
  with no executable kept.

### Out of scope

Large pages for the kept images (a separate change), executables that are
not `'static`, and dropping one executable at a time under pressure.

## Capabilities

### Modified Capabilities

- `cells`: executables kept loaded; what "every frame returned" means with
  them.

## Impact

`crates/rhizo-mm/src/cow.rs`, `crates/rhizo-cell/src/{lib,images}.rs`,
`kernel/src/cell.rs`, `kernel/src/tests.rs`, `cells/src/bin/same_addr.rs`,
`bench/README.md`. The cold workloads (`workload.cpython_cold_us` most)
drop; `cell.spawn_us` and `cell.run_us` (a two-page executable) stay where
they are; no metric gets worse.
