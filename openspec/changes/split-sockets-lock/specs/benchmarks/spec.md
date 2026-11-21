## ADDED Requirements

### Requirement: System-call scaling metric
The 4-vCPU `bench-cell` boot SHALL report `cell.syscall_scaling_pct`: the
time of four `piper` cells per CPU (each writing and reading a byte
through its pipe 20,000 times) run on every CPU, times the number of
CPUs, over their time run on one CPU after the other, in percent (100 is
perfect scaling).

#### Scenario: Metric present
- **WHEN** `cargo xtask bench` runs on a VMM where the counter frequency is known
- **THEN** the results contain `cell.syscall_scaling_pct`
