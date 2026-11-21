# Spec Delta

## ADDED Requirements

### Requirement: Timer accuracy metric
The 4-vCPU boot of `cargo xtask bench` SHALL report `cell.timer_error_us`:
on each secondary CPU, the median of five 10 ms waits on that CPU's own
timer, its distance from 10 ms by the counter, the worst CPU's.

#### Scenario: Metric present
- **WHEN** `cargo xtask bench` runs on a VMM where the counter frequency is known
- **THEN** the results contain `cell.timer_error_us`
