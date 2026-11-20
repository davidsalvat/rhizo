# Spec Delta

## ADDED Requirements

### Requirement: CPython workload metrics
Where CPython is built, `cargo xtask bench` SHALL report the time for a
CPython cell to answer one agent request started cold from its executable
(`workload.cpython_cold_us`: interpreter start, imports, warm-up and
answer) and forked from a template frozen after the warm-up
(`workload.cpython_warm_us`), for every VMM that measures cells.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs with CPython built
- **THEN** the results contain `workload.cpython_cold_us` and `workload.cpython_warm_us` for guest `rhizo`
