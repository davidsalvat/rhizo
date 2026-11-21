## ADDED Requirements

### Requirement: Wasted wake-up metric
The server workload SHALL report `density.wasted_wakeups_pct`: of the cells
woken from the waiting set by a socket change during the load, the share,
in percent, whose next system call waited on descriptors again. Lower is
better.

#### Scenario: Metric present
- **WHEN** the server workload runs
- **THEN** the results contain `density.wasted_wakeups_pct` for that VMM, computed from the `io_woken` and `io_wasted` counts of its result line
