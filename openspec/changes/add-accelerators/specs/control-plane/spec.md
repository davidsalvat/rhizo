# Spec Delta

## ADDED Requirements

### Requirement: Accelerator commands
`rhizo` SHALL offer, over control protocol version 5, `up --accel SPEC`
(start or connect a broker), `accel ls` (devices, partitions, their
domains, budgets and use), `run --accel DEVICE:RIGHTS` with
`--accel-mem` and `--accel-time`, and `load --bind-accel LABEL=DEVICE`.
Every grant, binding and refusal SHALL be audited.

#### Scenario: Listing devices
- **WHEN** `rhizo up --accel fake` runs and a cell is started with `--accel fake:infer --accel-mem 1G`
- **THEN** `rhizo accel ls` shows the fake device bound to that cell's domain with 1 GiB reserved
