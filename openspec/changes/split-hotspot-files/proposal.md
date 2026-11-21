# Proposal

## Why

Almost every change edits the same two files: `kernel/src/cell.rs`
(5,621 lines: the cell host, the scheduler loop, gangs, faults, sockets,
the page cache, snapshots and the cell benchmarks) and
`kernel/src/tests.rs` (4,235 lines: every in-kernel test, listed once more
in one `suite!` list). They are the two most edited files of the
repository (`tests.rs` in 56 commits on `main`, `cell.rs` in 50), and every new in-kernel test
adds a line to the same list. Many agents are about to work on the cell
hierarchy (`add-cell-hierarchy`) at once; with these files as they are,
their pull requests conflict even when their concerns do not.

## What Changes

- **`kernel/src/cell.rs` becomes a module with one concern per file**
  under `kernel/src/cell/`: the host and its run loop, what the host asks
  of one cell, the scheduler loop, a cell's time slice, gangs, page
  faults, FP/SIMD switching, frames, services of system calls, sockets,
  executables kept loaded, the page cache, snapshots and templates,
  snapshot transfers, the agent, and the cell benchmarks
  (`kernel/src/cell/bench/`). `kernel/src/cell.rs` keeps the public
  types, boot-time setup and the module list. The public surface
  (`crate::cell::*`) is unchanged.
- **In-kernel tests by area.** Each area has its file under
  `kernel/src/tests/` with its tests and its own `ALL` list, in execution
  order. `kernel/src/tests.rs` concatenates the areas' lists in one fixed
  order at compile time, and keeps the harness self-checks and
  `tests::chosen`. A new test touches only its area's file; only a new
  area touches `kernel/src/tests.rs`.
- **Nothing else changes.** No code is rewritten: items move between
  files, and an item one file needs from another becomes `pub(super)`.
  The tests, their names and their order are the same (the areas are the
  contiguous runs of the old list), so `selfcheck=` and `shard=` choose
  the same tests as before.

## Out of scope

- Other frequently edited files. Checked with `git log --format=
  --name-only origin/main | sort | uniq -c | sort -rn`: after these two,
  the most edited code files are `xtask/src/bench.rs`, `bench/baseline.json`
  and `kernel/src/control.rs` (being changed by `measure-control-path`),
  `xtask/src/main.rs` (already a 5-line binary over `xtask/src/lib.rs`),
  `cells/expected.rs` (one table read by both the kernel and the host's
  differential check: splitting it is not mechanical) and
  `crates/rhizo-abi/src/lib.rs` (the system-call dispatch, already split
  into modules by concern). The rest are documentation.
- Any change of behavior, of a test, or of a benchmark.

## Capabilities

### Modified Capabilities

- `developer-feedback`: in-kernel tests are grouped by area, each listing
  its own tests, so adding a test touches one file.

## Impact

- `kernel/src/cell.rs` and `kernel/src/cell/**`, `kernel/src/tests.rs`
  and `kernel/src/tests/*`.
- `AGENTS.md` (code map, hotspots) and `.agents/skills/kernel-test/SKILL.md`
  (where a new in-kernel test goes).
- No change to `xtask`, CI, specs of behavior, or benchmarks. The
  should-panic self-check still panics in `kernel/src/tests.rs`, so its
  expected output is unchanged.
