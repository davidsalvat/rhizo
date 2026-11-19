# Spec Delta

## ADDED Requirements

### Requirement: Real workload metrics
`cargo xtask bench` SHALL report the time for a real program with an
expensive warm-up to answer one request when started from its executable
and when forked from a template frozen after the warm-up.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs on a VMM where the counter frequency is known
- **THEN** the results contain `workload.cold_us` and `workload.warm_us`
