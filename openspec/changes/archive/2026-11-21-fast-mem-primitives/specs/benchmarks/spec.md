# Spec Delta

## ADDED Requirements

### Requirement: Snapshot export and import metrics
`cargo xtask bench` SHALL report, from the cell benchmark, the time to
export the 8 MiB `ballast` snapshot to bytes with its checksum
(`cell.snapshot_export_us`) and the time to check and decode those bytes
(`cell.snapshot_import_us`), each the median of 7 rounds, on every VMM,
and counted in instructions under QEMU `-icount`.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs on any VMM
- **THEN** the results contain `cell.snapshot_export_us` and `cell.snapshot_import_us` for guest `rhizo` on that VMM

#### Scenario: Counted under icount
- **WHEN** `cargo xtask bench --icount-only` runs
- **THEN** the results contain `cell.snapshot_export_insns` and `cell.snapshot_import_insns`
