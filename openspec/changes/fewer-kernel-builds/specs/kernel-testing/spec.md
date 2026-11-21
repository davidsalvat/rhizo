## ADDED Requirements

### Requirement: Harness self-checks chosen at boot
The in-kernel test build SHALL contain every harness self-check (a case
that must end the run in one precise way). Booted with `selfcheck=NAME`
it SHALL run one passing case and then self-check `NAME` instead of its
suite, and SHALL fail the run when `NAME` is unknown. `cargo xtask test`
SHALL run each self-check (step `ktest-<NAME>`) on the test kernel that
runs the in-kernel tests.

#### Scenario: A self-check without a build of its own
- **WHEN** `cargo xtask test --only ktest-page-fault` runs after `cargo xtask test --only ktest`, with no source changed
- **THEN** no kernel is compiled, the test kernel boots with `selfcheck=page-fault`, the null write is reported as a page fault, and the step passes

#### Scenario: Unknown self-check
- **WHEN** the test kernel boots with `selfcheck=bogus`
- **THEN** it panics with `unknown self-check` and the run fails

#### Scenario: No self-check named
- **WHEN** the test kernel boots without `selfcheck=`
- **THEN** it runs every in-kernel test and no self-check

### Requirement: Test code only in the test build
In-kernel tests and harness self-checks SHALL be compiled only into kernels
built with the test feature (`ktest`), chosen once per build, not per test
or self-check. Kernels built without it (the plain kernel, the control
kernel, the benchmark kernels) SHALL contain no test or self-check code,
and choosing tests at boot SHALL NOT change their images.

#### Scenario: A release kernel holds no self-check
- **WHEN** the control kernel `rhizo up` runs is built
- **THEN** it contains no self-check case, and `selfcheck=` on its command line has no effect
