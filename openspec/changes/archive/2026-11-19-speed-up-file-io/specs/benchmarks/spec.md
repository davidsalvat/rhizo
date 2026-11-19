# Spec Delta

## ADDED Requirements

### Requirement: File metrics are gated
The `ci` baseline SHALL include `cell.file_write_read_us` and
`cell.fork_with_files_us` for every VMM that measures cells, so the bench
gate fails a pull request that makes them slower beyond the threshold.

#### Scenario: Slower file input and output
- **WHEN** a change makes `cell.file_write_read_us` slower than its `ci` baseline by more than the timing threshold
- **THEN** `cargo xtask bench --env ci --check` fails and names the metric
