# Spec Delta

## ADDED Requirements

### Requirement: Density metrics
`cargo xtask bench` SHALL report `cell.idle_bytes`, the memory each of 1000
idle cells forked from a template costs, and `cell.suspended_bytes`, the
size of an idle cell's snapshot, as deterministic metrics.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs a `bench-cell` kernel
- **THEN** the results contain `cell.idle_bytes` and `cell.suspended_bytes`
