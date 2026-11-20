# Spec Delta

## ADDED Requirements

### Requirement: Python workload metrics
Where MicroPython is built, `cargo xtask bench` SHALL report the time for a
MicroPython cell to answer one request started cold from its executable
(`workload.python_cold_us`) and forked from a warm template
(`workload.python_warm_us`).

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs with MicroPython built
- **THEN** the results contain `workload.python_cold_us` and `workload.python_warm_us`
