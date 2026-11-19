# Spec Delta

## ADDED Requirements

### Requirement: Suspend and resume metrics
`cargo xtask bench` SHALL report, for a cell with 8 MiB of touched memory,
the time to suspend it, the time to resume it lazily until it can run, the
time to resume it eagerly, the time to resume it lazily and touch every
page, and the snapshot size in bytes.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs on a VMM where the counter frequency is known
- **THEN** the results contain `cell.suspend_us`, `cell.resume_us`, `cell.resume_eager_us`, `cell.resume_touch_us` and `cell.snapshot_bytes`
