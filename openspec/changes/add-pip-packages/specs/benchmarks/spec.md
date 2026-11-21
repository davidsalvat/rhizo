# Spec Delta

## ADDED Requirements

### Requirement: numpy metrics
`cargo xtask bench` SHALL report, for every VMM that measures cells and
where the packages are built, the time to start the packages' CPython from
the cell's files, import numpy, compute and exit
(`workload.numpy_cold_us`), and the time for the same computation in a
cell forked from a template frozen once numpy was imported
(`workload.numpy_warm_us`).

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs with `RHIZO_CPYTHON_SRC` set, natively, on a VMM where the counter frequency is known
- **THEN** the results contain `workload.numpy_cold_us` and `workload.numpy_warm_us` for guest `rhizo`
