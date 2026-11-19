# Spec Delta

## Purpose

Defines how Rhizo is compared with existing isolation mechanisms: measured
on the same machine, in the same run, with the same timing method.

## ADDED Requirements

### Requirement: Firecracker snapshot restore
`cargo xtask bench` SHALL measure, where Firecracker can run, the time from
launching a new Firecracker process to a restored Linux guest running, from
a full snapshot taken through Firecracker's API.

#### Scenario: Restore measured
- **WHEN** `cargo xtask bench` runs on an x86_64 host with KVM
- **THEN** the results contain `snapshot.restore_ms` and `snapshot.restore_api_ms` for guest `linux` on `firecracker`

#### Scenario: Not available
- **WHEN** Firecracker cannot run on the host
- **THEN** the comparison is listed as skipped with the reason, and no number is reported

### Requirement: Linux process baselines
`cargo xtask bench` SHALL measure, on a Linux host of the target
architecture, `fork` of a process holding 50 MiB of written memory and
spawning the same static program the cell benchmark runs.

#### Scenario: Process metrics
- **WHEN** `cargo xtask bench` runs on such a host
- **THEN** the results contain `process.fork_us` and `process.spawn_us`
