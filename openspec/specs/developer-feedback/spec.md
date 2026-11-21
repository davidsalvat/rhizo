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
over `N` VMs that together run each test exactly once. A failing VM's
reproduce command SHALL carry `RHIZO_SHARDS=N`. CI SHALL run the AArch64
in-kernel tests split this way.

#### Scenario: Two shares
- **WHEN** `RHIZO_SHARDS=2 cargo xtask test --only ktest` runs
- **THEN** two VMs run, each prints the tests it ran, and every test runs in one of them

#### Scenario: A share fails
- **WHEN** a test fails in one VM of `RHIZO_SHARDS=2 cargo xtask test --only ktest`
- **THEN** the reproduce command printed starts with `RHIZO_SHARDS=2`

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
tests and the crate dependency rules (`cargo xtask check`) in one job that
starts together with the jobs that build kernels or boot VMs. When it
fails, it SHALL cancel the run, so those jobs stop, and the run SHALL
fail.

#### Scenario: Lint warning
- **WHEN** a pull request introduces a lint warning
- **THEN** the quick job fails, the jobs that build kernels or boot VMs are cancelled, and `ci-ok` fails

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

### Requirement: Test steps split over machines
`cargo xtask test --part K/N` and `cargo xtask ci --part K/N` SHALL run
the `K`-th of `N` shares of the test steps, dealt by each step's estimated
cost and independently of any other option, so that for every `N` the `N`
parts together run every step exactly once, including steps added later.
`ci` SHALL run its lints and its x86_64-only steps in part 1 only. CI
SHALL split the AArch64 tests over two runners this way.

#### Scenario: Two runners
- **WHEN** `cargo xtask --arch aarch64 ci --part 1/2` and `--part 2/2` run on two machines
- **THEN** each test step of `cargo xtask --arch aarch64 ci` runs on exactly one of them, and only the first runs the lints

#### Scenario: A new step
- **WHEN** a step is added to `test` without a cost estimate
- **THEN** it runs in exactly one part

### Requirement: Affected steps for the local loop
`cargo xtask test --affected` SHALL run only the host tests of the crates
that the files changed against `origin/main` (committed, staged, unstaged
and untracked) reach through the crate dependency table, with every crate
that depends on them, and only the VM steps those crates reach. It SHALL
print the steps it selected. CI SHALL NOT use it.

#### Scenario: Command-line tool only
- **WHEN** only `cli/src/main.rs` changed and a developer runs `cargo xtask test --affected`
- **THEN** the host tests of `rhizo-cli` and the control-plane step run, and no in-kernel test runs

#### Scenario: Documentation only
- **WHEN** only Markdown files changed
- **THEN** no test runs and the selection printed is empty

### Requirement: VM steps an affected crate reaches
For `--affected`, the kernel's steps (boot check, in-kernel tests,
self-checks) SHALL run when the kernel or the cells are reached, the
differential check when the cells are, and the control plane when the
kernel or the command-line tool is. A changed file in `xtask`, or outside
every crate and the documentation, SHALL select every step.

#### Scenario: Cells changed
- **WHEN** only a file under `cells/` changed
- **THEN** the kernel's steps and the differential check run, and no host test runs

#### Scenario: Cargo files changed
- **WHEN** `Cargo.lock` changed
- **THEN** every step runs

### Requirement: In-kernel tests grouped by area
The in-kernel tests SHALL be grouped by area, one source file per area
under `kernel/src/tests/`, each listing its own tests in execution order.
The suite SHALL run the areas in one fixed order, each area's tests in its
order. Adding a test to an existing area SHALL change only that area's
file; only adding an area SHALL change the list of areas.

#### Scenario: A new test
- **WHEN** a change adds an in-kernel test to an existing area
- **THEN** the only test source file it changes is that area's file, and the test runs in `ktest` and `ktest-smp` at its place in the area's list

#### Scenario: Same tests after a regrouping
- **WHEN** tests are moved between files without changing any area's list or the order of areas
- **THEN** `cargo xtask test` runs the same tests, by the same names, in the same order
