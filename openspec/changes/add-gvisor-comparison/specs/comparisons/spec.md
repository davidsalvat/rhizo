# Spec Delta

## ADDED Requirements

### Requirement: gVisor sandboxes
`cargo xtask bench` SHALL measure, where gVisor's `runsc` is installed and
runs, a new gVisor sandbox running the cell benchmark's static `hello` to
its exit, and the restore of a checkpointed gVisor sandbox until it runs.

#### Scenario: gVisor measured
- **WHEN** `cargo xtask bench` runs on a host with `runsc` and root access
- **THEN** the results contain `sandbox.run_ms` and `snapshot.restore_ms` for guest `gvisor`

#### Scenario: gVisor not available
- **WHEN** `runsc` is missing or cannot run
- **THEN** the gVisor comparison is listed as skipped with the reason, and no number is reported
