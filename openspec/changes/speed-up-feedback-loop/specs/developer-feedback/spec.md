## ADDED Requirements

### Requirement: Test steps run at once
`cargo xtask test` SHALL build every kernel it needs before it starts any
VM, then run its steps concurrently on the host's CPUs, each step taking as
many host CPUs as its VM has virtual CPUs (at most half the host). It SHALL
report each step with its time as it ends and show a step's console output
when that step fails. `RHIZO_JOBS=N` SHALL set the host CPUs it uses. Steps
on VMMs that share a host device, and runs waiting for a debugger, SHALL
run one at a time.

#### Scenario: Several steps
- **WHEN** `cargo xtask test` runs on QEMU on a host with several CPUs
- **THEN** the in-kernel tests, the in-kernel tests on several CPUs, the host tests and the self-checks run at the same time, and a failure still names the step and its reproduce command

#### Scenario: One CPU
- **WHEN** `RHIZO_JOBS=1` is set
- **THEN** the steps run one after another with their output shown as before

### Requirement: Split in-kernel tests
`RHIZO_SHARDS=N` SHALL split the in-kernel tests of `ktest` and `ktest-smp`
over `N` VMs that together run each test exactly once.

#### Scenario: Two shares
- **WHEN** `RHIZO_SHARDS=2 cargo xtask test --only ktest` runs
- **THEN** two VMs run, each prints the tests it ran, and every test runs in one of them
