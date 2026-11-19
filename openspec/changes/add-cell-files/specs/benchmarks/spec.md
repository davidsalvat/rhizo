# Spec Delta

## ADDED Requirements

### Requirement: File metrics
`cargo xtask bench` SHALL report the time for a cell to create, write and
read back 1 MiB of file data, and the time to fork a cell holding 16 MiB of
files.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs on a VMM where the counter frequency is known
- **THEN** the results contain `cell.file_write_read_us` and `cell.fork_with_files_us`

### Requirement: Robust cell figures
Each cell figure of a `bench-cell` run SHALL be the minimum, over several
rounds, of the median of each round.

#### Scenario: One slow round
- **WHEN** one round of a cell benchmark is slowed down by its host
- **THEN** the reported figure is that of the fastest round
