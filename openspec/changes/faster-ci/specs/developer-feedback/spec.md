## ADDED Requirements

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
