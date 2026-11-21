# Design

## Context

`kernel/src/cell.rs` (5,621 lines) and `kernel/src/tests.rs` (4,235
lines) are the files most changes edit, and the next milestone
(`add-cell-hierarchy`) is planned as many parallel pull requests. Both
files already fall into concerns: `cell.rs` is a sequence of sections
(sockets, images, frames, FP switching, faults, the scheduler loop,
gangs, `impl Host`, benchmarks), and `tests.rs` has section comments per
area, but every test is also listed in one `suite!` macro, so two pull
requests that each add a test always touch the same lines.

## Goals

- One concern per file, each small enough to review on its own.
- A new in-kernel test touches its area's file only.
- No behavior change: the same public surface, the same tests in the
  same order, the same machine code on hot paths.

## Decisions

### D1. Moves only, with `pub(super)` where a sibling needs an item

Each submodule starts with `use super::*;`, and the module root
(`cell.rs`, `tests.rs`) holds the imports the code had and imports each
submodule's items (`use host::*;`, `pub use` for those with public
items), so every name resolves as it did in the single file. An item, a
field or a method another file uses becomes `pub(super)` (visible in the
module, not outside it); nothing else changes in the code. Methods of
`Host` are spread over several `impl Host` blocks by concern (the host
and its loop, operations on one cell, snapshots and templates, snapshot
transfers).

Alternatives: explicit imports per file (precise, but every new
cross-file use edits an import list, which brings back the conflicts);
`include!` of fragments (no real modules, so privacy and tooling do not
see the split).

The module roots stay `cell.rs` and `tests.rs`, their submodules in
`cell/` and `tests/` (the Rust 2018 layout): the should-panic self-check
still panics in `kernel/src/tests.rs`, so `xtask`'s expected output is
unchanged, and paths in the specs and the CI classification tests still
name existing files.

The cell benchmarks (`bench-cell`) move to `cell/bench.rs` and
`cell/bench/`, compiled only with that feature; the `#[cfg(feature =
"bench-cell")]` each of their items carried becomes the module's, and
the profile breakdowns are a module compiled with `profile`.

### D2. Test areas are contiguous runs of the old list

Each area file has `pub(super) const ALL: &[TestCase]`, its tests in
execution order. `tests.rs` lists the areas in `AREAS` and concatenates
them at compile time into one array (`const fn concat`, sized by
`const fn count`), so `ALL` stays one `&'static [TestCase]`: the runner,
`chosen`, `selfcheck=` and `shard=` see the same slice as before.

The areas are the contiguous runs of the old `suite!` list, cut where its
concern changes, so the concatenation is the old list exactly: same
names, same order, the same test in the same position for `shard=I/N`.
A test whose function sat in another section of the old file (for
example `suspend_between_parallel_runs_is_invisible`, among the SMP
tests but run with the snapshot tests) moves to the area it runs in.
`micropython_in_a_cell` is an area of its own because the network tests
run between it and the CPython tests.

The Node.js and package builds keep their own lists (`node::ALL`,
`packages::ALL`), in modules compiled only with their feature. The
self-checks stay in `tests.rs`: adding one also edits `xtask`, and they
change rarely.

Test names are the function names (`ktest_case!` stringifies its path),
so each area lists bare names, never `area::name`.

## Verification

- The test list, by name and order, from the console of `ktest` and
  `ktest-smp` (`RHIZO_JOBS=1`) on both architectures, before and after.
- The release `bench-cell` and plain kernels, function by function
  (sizes after folding the new module paths back), before and after on
  both architectures, and the `-icount` benchmarks before and after.
- `cargo xtask ci` on both architectures.
