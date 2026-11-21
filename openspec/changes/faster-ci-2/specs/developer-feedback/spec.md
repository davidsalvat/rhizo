## MODIFIED Requirements

### Requirement: Quick checks first
CI SHALL run formatting, lints for every crate and architecture, host
tests and the crate dependency rules (`cargo xtask check`) in one job that
starts together with the jobs that build kernels or boot VMs. When it
fails, it SHALL cancel the run, so those jobs stop, and the run SHALL
fail.

#### Scenario: Lint warning
- **WHEN** a pull request introduces a lint warning
- **THEN** the quick job fails, the jobs that build kernels or boot VMs are cancelled, and `ci-ok` fails

## ADDED Requirements

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
