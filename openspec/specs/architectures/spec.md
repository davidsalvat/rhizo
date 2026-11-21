# architectures Specification

## Purpose
Defines which CPU architectures Rhizo supports, what each must provide behind
the `arch/` boundary, and how both are kept working.

## Requirements

### Requirement: Supported architectures
Rhizo SHALL support AArch64 as its primary architecture and x86_64 as a second
architecture. `cargo xtask` commands SHALL default to AArch64 and accept
`--arch x86_64`.

#### Scenario: Default architecture
- **WHEN** a developer runs `cargo xtask run` without `--arch`
- **THEN** the AArch64 kernel is built and booted

#### Scenario: Second architecture
- **WHEN** a developer runs `cargo xtask --arch x86_64 test`
- **THEN** the x86_64 kernel is built and every test step runs against it

### Requirement: Architecture code stays behind arch
Instruction-set-specific code (assembly, system registers, port or MMIO
device access, page-table entry encodings) SHALL live in `kernel/src/arch/`
or in a format type of `rhizo-mm`; shared kernel code SHALL use only the
documented `arch` interface.

#### Scenario: Shared in-kernel tests
- **WHEN** the shared in-kernel tests run on either architecture
- **THEN** they pass without architecture-specific conditions; tests that need them live in `arch/<cpu>/tests.rs`

### Requirement: Both architectures pass every spec
Every requirement of the kernel specs SHALL hold on both architectures, and CI
SHALL run the full test suite for each on every pull request that changes
code and on every push to main.

#### Scenario: Pull request
- **WHEN** a pull request that changes code is opened
- **THEN** CI runs `cargo xtask ci` for AArch64 and for x86_64, and both must pass

### Requirement: Page-table formats share one implementation
Page-table creation, mapping, translation and unmapping SHALL be implemented
once and parameterized by the entry format, and their behavior tests SHALL
run for every format.

#### Scenario: Same behavior on both formats
- **WHEN** the page-table tests run
- **THEN** each behavior test runs once with the x86_64 format and once with the AArch64 format
