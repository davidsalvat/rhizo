---
name: kernel-test
description: Add or debug tests for Rhizo, on the host (rhizo-hal, rhizo-core, rhizo-mm) or inside the kernel under QEMU, including self-checks that must fail.
---

# Testing in Rhizo

Pick the cheapest level that can observe the behavior:

1. **Host test** (milliseconds): logic in `crates/*`. Put the test next to the
   code in a `#[cfg(test)] mod tests`. Use the doubles in
   `rhizo_hal::testing` (`RecordingConsole`, `FakePhysicalMemory`,
   `RecordingTlb`, `ManualCycleCounter`). Use `proptest` for invariants;
   the seed is fixed by `.cargo/config.toml`, so failures reproduce.
   Run: `cargo test -p <crate> <name>`.
2. **In-kernel test** (seconds): needs real page tables, CPU state or
   devices. Add a `fn` to its area's file in `kernel/src/tests/` and list
   it in that file's `ALL`; a new area is a new file, listed in `AREAS` in
   `kernel/src/tests.rs`. Run: `cargo xtask test --only ktest`.
3. **Self-check** (a run that must fail in a precise way): the test kernel
   runs one when booted with `selfcheck=<name>`, so it needs no build of its
   own. Add a `("<name>", self_check!(<case>))` row to `SELF_CHECKS` in
   `kernel/src/tests.rs`, and a row to `SELF_CHECKS` in `xtask/src/lib.rs`
   (step `ktest-<name>`, option `<name>`) with the expected result and
   output. Run: `cargo xtask test --only ktest-<name>`.

Test observable behavior through public interfaces. Never weaken, skip or
delete a test to make it pass; if the spec changed, change the spec first.

Reading a failure: `xtask` prints `FAILED <kind>` with `where`, `expected`,
`actual`, `hint` and `reproduce`. For an exception, run the `addr2line`
command from the hint to find the source line.
