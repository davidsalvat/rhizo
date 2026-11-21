# Proposal

## Why

`cargo xtask test` builds the kernel once per cargo feature set it boots,
and every set compiles the whole `kernel` crate again (about 5 s each in
a debug build). The eight harness self-checks (`ktest-should-panic`,
`ktest-hang`, `ktest-page-fault`, `ktest-stack-overflow`,
`ktest-invalid-opcode`, `ktest-oom`, `ktest-user-read`,
`ktest-user-exec`) are eight features, so after any edit to kernel code
`test` compiles 11 kernels per architecture: the plain kernel, the test
kernel, the control kernel and the eight self-checks. `cargo xtask ci`
also runs clippy on 15 feature sets, eight of them the self-checks. The
self-check kernels differ from the test kernel only in which test cases
they list, which a boot option can choose as well as a build.

## What Changes

- **Self-checks chosen at boot.** The test kernel (feature `ktest`) holds
  every self-check case. Booted with `selfcheck=NAME` (`should-panic`,
  `hang`, `page-fault`, `stack-overflow`, `invalid-opcode`, `oom`,
  `user-read`, `user-exec`) it runs that check instead of its suite; an
  unknown name fails the run. The eight `ktest-*` features are removed.
- **One test kernel per run.** `cargo xtask test` builds the test kernel
  once and boots it for `ktest`, `ktest-smp` (which already differed only
  in CPUs) and every self-check, adding `selfcheck=` for those. The step
  names (`--only ktest-page-fault`, …), the expected results and outputs
  and the reproduce commands stay as they are.
- **Fewer clippy runs in `ci`.** The self-check code is compiled, and
  linted, in the `ktest` set; the eight self-check sets go.
- **Test code behind one feature.** `ktest` stays the single build-time
  "test-capable" switch: no other kernel (plain, `control`, `bench-cell`,
  the ones `rhizo up` and the benchmarks run) contains test or self-check
  code. Their images are byte-identical to before.

A full `test` or `ci` then compiles 3 kernels per architecture (plain,
`ktest`, `control`), plus one per large embedded image (`node`,
`packages`) only where its environment variable is set.

## Out of scope

- Features that stay compile-time, with the reasons in `design.md`:
  `ktest` itself, `control`, `bench-cell`, `node`, `packages`, `profile`,
  `sampling` and `cells`.
- The benchmark kernels (`bench`): they are release builds of their own
  (plain and `bench-cell`), unchanged.
- The CI workflows.

## Capabilities

### Modified Capabilities

- `kernel-testing`: self-checks are runs of the test kernel chosen with
  `selfcheck=NAME`; one test kernel per architecture serves the suite and
  every self-check; test code exists only in builds with the test feature.
- `developer-feedback`: `cargo xtask test` compiles each kernel it needs
  once, and only the kernels that differ in what they contain.

## Impact

- `kernel/Cargo.toml` (features), `kernel/src/main.rs`,
  `kernel/src/tests.rs`, `kernel/src/arch/*/{cpu,user}.rs` (cfg of the
  self-check helpers).
- `crates/rhizo-core/src/options.rs`: `self_check`, read apart from
  `BootOptions` so kernels without tests never parse it.
- `xtask/src/lib.rs` (`test`, `ci`, `SELF_CHECKS`), `xtask/src/vmm.rs`
  (`RunOptions::selfcheck`, the `selfcheck=` argument on every VMM), and
  the `RunOptions` literals elsewhere in `xtask`.
- `.agents/skills/kernel-test/SKILL.md`: how to add a self-check.
- No benchmark kernel changes: no metric moves.
