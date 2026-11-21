## ADDED Requirements

### Requirement: Host operation stall metrics
The 4-vCPU `bench-cell` boot of `cargo xtask bench` SHALL report
`cell.host_op_stall_us`: the median, over rounds in which the host forks a
spinning cell, kills the child, and suspends and resumes the spinner from
the run's stop condition, of the longest time a running `progress` cell
went without running. It SHALL also report `cell.host_op_stall_stopped_us`,
the same with each round made between runs, for comparison.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs on a VMM where the counter frequency is known
- **THEN** the results contain `cell.host_op_stall_us` and `cell.host_op_stall_stopped_us`
