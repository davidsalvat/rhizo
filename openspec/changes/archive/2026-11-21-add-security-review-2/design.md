# Design

## D1. Fuzz the libraries, attack the kernel with cells

The kernel's logic lives in host-testable crates, so the fuzzers are
`proptest` properties over them with the existing test doubles
(`FakePhysicalMemory`, `CountingFrames`, a kernel half holding a secret),
not a fuzzer of the kernel binary. Fixed seeds keep pull requests
reproducible (`.cargo/config.toml`); the weekly job draws a seed and runs
4096 cases. Alternatives: `cargo fuzz` (libFuzzer) needs nightly and a
separate harness per target, and does not run in `cargo xtask check`; a
fuzzer inside the kernel (as `fuzz` already is) cannot shrink a failure.

Inputs start from valid encodings with a few bytes changed (weighted to
headers and records), plus fully random bytes behind a valid header:
random bytes alone almost never pass the first checks.

## D2. What "contained" means for a snapshot

A snapshot that `from_bytes` accepts must restore into a cell that (a)
changes no kernel memory and no kernel entry, (b) maps only user pages in
the lower half with W^X, (c) has every thread's program counter (and on
x86_64 thread pointer) below `USER_END`, (d) has files whose every
operation answers, (e) has socket records that import, and (f) gives every
frame back. Checked on x86_64 page tables, where the cell's root shares
the kernel half (the harder case).

## D3. Entry points checked where every start passes

`start_at` is the one place every program start goes through (static
spawn, kept images, `exec` with an interpreter), so it refuses a first
instruction at or past `USER_END`, like S7 and S8 did for segments and
snapshots. Checking in the ELF parser instead would miss the interpreter's
load address, which only `map_program` knows.

## D4. Races are run, not modelled

`racer` runs each race thousands of times on four CPUs; it can show a bug
only when the interleaving happens. A model of the cell lock and the
shootdown protocol would be stronger and is left open.
