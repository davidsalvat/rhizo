# Spec Delta

## ADDED Requirements

### Requirement: Multi-core metrics
`cargo xtask bench` SHALL also boot the `bench-cell` kernel with 4 vCPUs on
every VMM that measures cells and report `cell.parallel_us` (time per
CPU-bound cell when four such cells per CPU run on every CPU),
`cell.scaling_pct` (that batch's time on every CPU, times the number of
CPUs, over its time on one CPU, in percent: 100 is perfect scaling) and
`cell.ipi_wake_us` (handing work to an idle CPU, waking it with an
interrupt, until it is done).

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs on a VMM where the counter frequency is known
- **THEN** the results contain `cell.parallel_us`, `cell.scaling_pct` and `cell.ipi_wake_us`
