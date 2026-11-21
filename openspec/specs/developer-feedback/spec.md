# developer-feedback Specification

## Purpose
Defines the feedback developer tooling gives to its users, most of whom are AI
coding agents: how fast it is, what every failure report contains, the
machine-readable output, and how failures are reproduced.

## Requirements

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

### Requirement: Pull requests classified by path
CI SHALL classify each pull request by the paths it changes since its
merge base, with a script that has tests of its own: *docs* when every
path is documentation, a spec, an agent skill or an image (`docs/`,
`openspec/`, `.agents/`, `*.md`, `bench/*.svg`), *host-tools* when the
others are only host tools the kernel never links (`cli/`), and *code*
otherwise, including no paths.

#### Scenario: Kernel change
- **WHEN** a pull request changes `kernel/src/cell.rs` and a Markdown file
- **THEN** it is classified *code*

### Requirement: CI runs by change class
A docs pull request SHALL run only the validation of the specs and of the
changes it touches, the check of relative Markdown links, and the image
checks when an image or its generator changed. A host-tools pull request
SHALL also run the quick checks and the control-plane step. A code pull
request, and every push to main, SHALL run every job.

#### Scenario: Design-only pull request
- **WHEN** a pull request changes only files under `openspec/changes/`
- **THEN** CI validates the specs and that change, checks the Markdown links, and builds no kernel

#### Scenario: Code pull request
- **WHEN** a pull request is classified *code*
- **THEN** CI runs every job

### Requirement: Quick checks first
CI SHALL run formatting, lints for every crate and architecture, host
tests and the crate dependency rules (`cargo xtask check`) in one job
before the jobs that build kernels or boot VMs, and those jobs SHALL wait
for it and not run when it fails.

#### Scenario: Lint warning
- **WHEN** a pull request introduces a lint warning
- **THEN** the quick job fails and no kernel is built or booted for that run

### Requirement: One aggregate CI result
CI SHALL end with one job, `ci-ok`, that succeeds only when every other job
of the run either succeeded or was skipped because the change's class does
not need it, and fails when any job failed or was cancelled.

#### Scenario: A heavy job fails
- **WHEN** the AArch64 tests fail on a code pull request
- **THEN** `ci-ok` fails

#### Scenario: Docs-only pull request
- **WHEN** a docs pull request passes its checks and the other jobs are skipped by its class
- **THEN** `ci-ok` succeeds

### Requirement: Flaky jobs recorded
When a CI or benchmark run succeeds on a re-run of the same commit, every
job that failed in an earlier attempt SHALL be recorded in an issue titled
`flaky: ` and its first failure line (or its job and step), opened or
commented on, with links to the failed job and the passing attempt. CI
SHALL NOT retry a job or skip a test automatically.

#### Scenario: Passes on a re-run
- **WHEN** a job fails, is re-run on the same commit and the run then succeeds
- **THEN** an issue named after that job's failure holds a link to both attempts, and the workflows are unchanged
