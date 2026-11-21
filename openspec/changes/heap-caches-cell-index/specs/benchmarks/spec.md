## ADDED Requirements

### Requirement: Host operations at the server's target
The server workload SHALL report the median time of the host's
operations made during its load, with every worker live:
`density.suspend_us` (suspending a worker), `density.fork_us` (forking a
computing worker) and `density.kill_us` (killing the previous fork).

#### Scenario: Metrics present
- **WHEN** the server workload runs
- **THEN** the results contain `density.suspend_us`, `density.fork_us` and `density.kill_us` for that VMM, from its breakdown line
