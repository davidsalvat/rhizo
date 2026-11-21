## ADDED Requirements

### Requirement: Few kernel builds
`cargo xtask test` SHALL compile one kernel per set of contents it needs,
never one per test step: what differs only in what runs (CPUs, a share of
the tests, a self-check) SHALL be chosen on the kernel command line. A
full `test` or `ci` SHALL compile at most 3 kernels per architecture,
plus one per large embedded image whose variable (`RHIZO_NODE`,
`RHIZO_CPYTHON_SRC`) is set.

#### Scenario: Edit loop
- **WHEN** a developer edits a kernel source file and runs `cargo xtask test`
- **THEN** the run compiles the plain, test and control kernels once each, and every self-check, `ktest` and `ktest-smp` boots the same test kernel

#### Scenario: Self-checks after the suite
- **WHEN** `cargo xtask test --only ktest-hang` runs right after `cargo xtask test --only ktest` with no source changed
- **THEN** no kernel is compiled
