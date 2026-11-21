# Spec Delta

## ADDED Requirements

### Requirement: KPTI metrics
`cargo xtask bench` SHALL report `cell.syscall_ns`, the round trip of the
cheapest system call (`getpid`) measured by a cell, in the kernel's
default mode, and the cost of KPTI forced on: `cell.syscall_kpti_ns`,
`cell.switch_kpti_ns` (as `cell.switch_ns`) and `cell.run_kpti_us` (as
`cell.run_us`), for every VMM that measures cells.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs a `bench-cell` kernel where the counter frequency is known
- **THEN** the results contain `cell.syscall_ns`, `cell.syscall_kpti_ns`, `cell.switch_kpti_ns` and `cell.run_kpti_us`
