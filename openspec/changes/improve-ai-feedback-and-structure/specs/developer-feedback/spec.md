# Spec Delta

## Purpose

Defines the feedback developer tooling gives to its users, most of whom are AI
coding agents: how fast it is, what every failure report contains, the
machine-readable output, and how failures are reproduced.

## ADDED Requirements

### Requirement: Fast check command
`cargo xtask check` SHALL run formatting, lints for every crate, host tests and
the crate dependency rules without booting a virtual machine, and SHALL print
how long it took.

#### Scenario: Quick verification
- **WHEN** a developer runs `cargo xtask check` on a built workspace
- **THEN** it reports success or the first failure without starting any VMM, and prints its duration

### Requirement: Actionable failure reports
Every failure reported by `xtask` SHALL state the kind of check, what failed,
where (`file:line` when known, otherwise the command), what was expected, what
was observed, and an exact command that reproduces it.

#### Scenario: In-kernel test failure
- **WHEN** an in-kernel test panics at `kernel/src/tests.rs:42`
- **THEN** the report names the test, gives `kernel/src/tests.rs:42` as the location, includes the panic message, and gives a command that reruns that suite

#### Scenario: Self-check failure
- **WHEN** a self-check ends with a different result than expected
- **THEN** the report shows the expected and actual result and the command that reruns only that self-check

#### Scenario: Benchmark regression
- **WHEN** the benchmark gate fails
- **THEN** the report names the metric, the baseline and current values, the allowed threshold, and the command to update the baseline if the change is intended

### Requirement: Machine-readable output
`check`, `test`, `ci` and `bench` SHALL accept `--json` and then print one JSON
object per line for every step start, failure and final summary, with the same
fields as the human report.

#### Scenario: JSON failure
- **WHEN** `cargo xtask check --json` hits a failing host test
- **THEN** standard output contains a line with `"event":"failure"` and the fields kind, what, where, expected, actual and reproduce

### Requirement: Reproducible property tests
Property tests SHALL use a fixed seed unless one is given explicitly, and a
reported property-test failure SHALL include the seed and the command to
reproduce it.

#### Scenario: Same cases everywhere
- **WHEN** the host tests run twice on different machines without overriding the seed
- **THEN** the property tests generate the same cases both times

### Requirement: Rerun a single check
`cargo xtask test --only <name>` SHALL run just the named step (boot check,
in-kernel tests, or one self-check).

#### Scenario: Rerun one self-check
- **WHEN** a developer runs `cargo xtask test --only ktest-page-fault`
- **THEN** only that self-check runs and reports its result
