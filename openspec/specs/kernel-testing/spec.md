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
The in-kernel test command SHALL stop a test VM that does not exit within a
configured time limit and report the run as failed.

#### Scenario: Kernel hangs during tests
- **WHEN** a test VM produces no exit status before the time limit
- **THEN** the VM is terminated
- **AND** the command reports a timeout and exits with a non-zero status

### Requirement: Single developer entry point
The repository SHALL provide one documented command each to build the kernel,
run it under QEMU, run host tests and run in-kernel tests. Contributors SHALL
need only the pinned Rust toolchain and QEMU.

#### Scenario: Fresh checkout
- **WHEN** a contributor with the Rust toolchain and QEMU installed runs the documented test commands on a fresh checkout
- **THEN** the kernel builds and both host and in-kernel tests run without extra manual setup

### Requirement: Continuous integration gate
Every pull request SHALL be checked automatically for formatting, lint warnings,
host tests and in-kernel tests. Any failure SHALL mark the pull request as
failing.

#### Scenario: Lint warning in a pull request
- **WHEN** a pull request introduces a lint warning
- **THEN** the CI run for that pull request fails
