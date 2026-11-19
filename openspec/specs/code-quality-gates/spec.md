# code-quality-gates Specification

## Purpose
Defines the automated gates that keep the code base small, legally clean and
correctly structured: lints, dependency and license policy, unsafe-code
checking, mutation testing, and the dependency rules between crates.

## Requirements

### Requirement: Crate dependency rules
The workspace SHALL follow a documented table of allowed dependencies between
its crates, with no cycles, platform code only in the kernel crate, and the
hardware-interface crate depending on no other workspace crate. `xtask` SHALL
fail when a dependency outside the table exists.

#### Scenario: Forbidden dependency
- **WHEN** a library crate adds a dependency on the kernel crate
- **THEN** `cargo xtask check` fails naming both crates and the rule they break

### Requirement: Dependency license policy
CI SHALL reject dependencies under GPL, AGPL or LGPL licenses, dependencies
with known security advisories, and declared dependencies that are not used.

#### Scenario: Copyleft dependency
- **WHEN** a pull request adds a dependency licensed under GPL-3.0
- **THEN** the CI run fails naming the dependency and its license

### Requirement: Lints against speculative and debugging code
The workspace lints SHALL reject leftover debugging and placeholder code
(`dbg!`, `todo!`, `unimplemented!`), printing from library crates, and
undocumented `unsafe` blocks.

#### Scenario: Leftover dbg!
- **WHEN** a change leaves a `dbg!` call in a library crate
- **THEN** `cargo xtask check` fails at the lint step with its location

### Requirement: Unsafe heap code checked by Miri
CI SHALL run the heap allocator's host tests under Miri and fail on any
undefined behavior it reports.

#### Scenario: Undefined behavior
- **WHEN** a change makes the heap read a freed block
- **THEN** the Miri job fails with the location of the invalid access

### Requirement: Mutation testing
`cargo xtask mutants` SHALL run mutation testing on the hardware-independent
crates and list the mutants that no test detects; it SHALL run weekly in CI.

#### Scenario: Weak test found
- **WHEN** replacing a comparison in the frame allocator does not make any test fail
- **THEN** the mutation report lists that mutant with its file and line
