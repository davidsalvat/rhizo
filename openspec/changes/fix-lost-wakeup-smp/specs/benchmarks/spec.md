# Spec Delta

## ADDED Requirements

### Requirement: Server templates frozen asleep
The server workload SHALL freeze each `worker` template only once it
sleeps, so that every worker forked from it starts its operations at once.

#### Scenario: Four CPUs
- **WHEN** `cargo xtask --arch x86_64 bench --workload server --cpus 4` runs on QEMU `microvm`
- **THEN** 10,000 workers run on 4 vCPUs, each exits with 0, and the load ends on time

### Requirement: Server workload on several CPUs
The server VM SHALL have 1 virtual CPU, or the number `--cpus` gives (1 to
4) for a run that neither checks nor updates the baseline.

#### Scenario: Several CPUs against the baseline
- **WHEN** `cargo xtask bench --cpus 4 --check` runs
- **THEN** it is refused before anything runs
