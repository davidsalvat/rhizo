# kernel-testing Specification

## Purpose
Defines how Rhizo's behavior is verified automatically, both on the developer's
host and inside a virtual machine, and how results are reported to developers
and continuous integration.

## Requirements

### Requirement: Hardware-independent code is testable on the host
Kernel logic that does not touch hardware SHALL compile and run its unit tests
on the developer's host operating system with the standard Rust test command,
without QEMU and without the kernel target.

#### Scenario: Host tests run without a VM
- **WHEN** a developer runs the host test command on a machine without QEMU installed
- **THEN** the hardware-independent tests compile, run and report their results

### Requirement: Kernel core reaches hardware only through interfaces
Kernel core code SHALL access the console and power control only through
interfaces that can be implemented by a test double on the host. Platform code
implementing those interfaces SHALL be kept separate from core code.

#### Scenario: Core runs against a host double
- **WHEN** a host test supplies an in-memory console implementation to core code that writes output
- **THEN** the test can read back exactly what the core code wrote

### Requirement: In-kernel tests run inside a virtual machine
A test build of the kernel SHALL boot under QEMU, run every registered in-kernel
test case, print each test's name and result to the serial console, and exit the
VM with a status that tells the host whether all tests passed.

#### Scenario: All tests pass
- **WHEN** the in-kernel test command is run and every test case passes
- **THEN** each test name is printed with an `ok` result
- **AND** the command exits with status 0

#### Scenario: A test fails
- **WHEN** an in-kernel test case panics or fails an assertion
- **THEN** the failing test name and the failure message are printed
- **AND** the command exits with a non-zero status

### Requirement: Hung test runs are terminated
The in-kernel test command SHALL stop a test VM that prints nothing for a
configured time limit and report the run as failed; a run that keeps
printing progress is not stopped however long it takes.

#### Scenario: Kernel hangs during tests
- **WHEN** a test VM produces no output and no exit status for the time limit
- **THEN** the VM is terminated
- **AND** the command reports a timeout and exits with a non-zero status

#### Scenario: Long suite
- **WHEN** a test VM keeps printing test results for longer than the time limit in total
- **THEN** it is not stopped

### Requirement: Single developer entry point
The repository SHALL provide one documented command each to build the kernel,
run it under QEMU, run host tests and run in-kernel tests. Contributors SHALL
need only the pinned Rust toolchain and QEMU.

#### Scenario: Fresh checkout
- **WHEN** a contributor with the Rust toolchain and QEMU installed runs the documented test commands on a fresh checkout
- **THEN** the kernel builds and both host and in-kernel tests run without extra manual setup

### Requirement: Continuous integration gate
Every pull request that changes code SHALL be checked automatically for
formatting, lint warnings, host tests and in-kernel tests; one that changes
only host tools SHALL be checked for formatting, lint warnings and host
tests; one that changes only documentation SHALL be checked for valid specs
and links. Any failure SHALL mark the pull request as failing.

#### Scenario: Lint warning in a pull request
- **WHEN** a pull request introduces a lint warning
- **THEN** the CI run for that pull request fails

### Requirement: In-kernel tests on several CPUs
`cargo xtask test` SHALL also run the whole in-kernel test suite on a VM
with 4 vCPUs (step `ktest-smp`), passing the CPU count each VMM takes
(QEMU `-smp`, Firecracker `vcpu_count`, Cloud Hypervisor `--cpus boot=`),
and SHALL fail unless the kernel reports `cpus: 4 online` and every test
passes. CI SHALL run it for both architectures on QEMU, and on Firecracker
and Cloud Hypervisor where KVM is available.

#### Scenario: Rerun on several CPUs
- **WHEN** a developer runs `cargo xtask test --only ktest-smp`
- **THEN** the in-kernel tests run on 4 vCPUs and the step reports their result

### Requirement: A share of the in-kernel tests
An in-kernel test build booted with `shard=I/N` (`0 <= I < N`) SHALL run
only the tests dealt to share `I`, reporting how many it runs, and SHALL
ignore a malformed value. The tests SHALL be ranked by their estimated
cost, most expensive first (1 unless given, ties by position across all
test suites), and dealt by rank to shares `0` to `N-1`, then `N-1` to
`0`, and again; each share SHALL run its tests in suite order. The deal
SHALL depend only on the suites and `N`.

#### Scenario: Shares cover every test once
- **WHEN** the test build boots `N` times with `shard=0/N` to `shard=N-1/N`
- **THEN** every test runs in exactly one of the boots

#### Scenario: Expensive tests apart
- **WHEN** the two most expensive tests sit at positions of the same parity and the build boots with `shard=0/2` and `shard=1/2`
- **THEN** they run in different boots, each boot in suite order

### Requirement: Tests do not depend on their order
An in-kernel test SHALL pass whether or not the tests before it ran: work
done once per boot (such as bringing up the network card) SHALL happen at
boot, and a test that compares free memory SHALL warm the kernel heap
first. A test that compares memory before and after itself SHALL take
both measures with no connection through the network card still closing,
waiting a bounded time for those an earlier test closed, and SHALL fail
when one never finishes closing.

#### Scenario: Memory check first in its VM
- **WHEN** a test that checks every frame is returned runs first after boot
- **THEN** it passes as it does after the other tests

#### Scenario: A connection still closing from an earlier test
- **WHEN** a test that checks every heap allocation is returned runs right after a test whose connection through the card is still closing
- **THEN** the connection finishes closing before the first measure, and the check counts only the test's own allocations

#### Scenario: Any split of the suite
- **WHEN** `ktest` or `ktest-smp` runs with `RHIZO_SHARDS` from 1 to 4
- **THEN** every VM passes

### Requirement: Node.js tests in their own VM
Where Node.js is packed (`RHIZO_NODE`), `cargo xtask test` SHALL run the
Node.js in-kernel tests in a kernel of their own, the only one embedding
Node.js, on a VM with 1 GiB and 2 vCPUs (step `ktest-node`), and SHALL
fail unless the kernel reports `cpus: 2 online` and every test passes.
Without `RHIZO_NODE` the step SHALL say why it did not run. CI SHALL run
it on both architectures.

#### Scenario: Rerun the Node.js tests
- **WHEN** a developer runs `cargo xtask test --only ktest-node` with `RHIZO_NODE` set
- **THEN** the Node.js tests run on 2 vCPUs with 1 GiB and the step reports their result

#### Scenario: Not downloaded
- **WHEN** a developer runs `cargo xtask test` without `RHIZO_NODE`
- **THEN** the step prints that the Node.js tests did not run and why, and the run passes

### Requirement: Package tests in their own VM
Where the packages are built (`RHIZO_CPYTHON_SRC`, natively), `cargo xtask
test` SHALL run the package in-kernel tests in a kernel of their own, the
only one embedding the packages, on a VM with 1 GiB and 2 vCPUs (step
`ktest-packages`), and SHALL fail unless the kernel reports `cpus: 2
online` and every test passes. Otherwise the step SHALL say why it did
not run. CI SHALL run it on both architectures.

#### Scenario: Rerun the package tests
- **WHEN** a developer runs `cargo xtask test --only ktest-packages` with `RHIZO_CPYTHON_SRC` set
- **THEN** the package tests run on 2 vCPUs with 1 GiB and the step reports their result

#### Scenario: Not built
- **WHEN** a developer runs `cargo xtask test` without `RHIZO_CPYTHON_SRC`
- **THEN** the step prints that the package tests did not run and why, and the run passes

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
