# Spec Delta

## ADDED Requirements

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
