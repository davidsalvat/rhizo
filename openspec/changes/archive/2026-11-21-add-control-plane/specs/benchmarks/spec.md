# Spec Delta

## ADDED Requirements

### Requirement: Control-plane metrics
`cargo xtask bench` SHALL report, on every VMM it measures, the round trip
of a `list` request from the host through a control kernel and back
(`control.roundtrip_us`), and the host-measured time of `run`, `suspend`,
`resume`, `fork` and `kill` on an idle cell (`control.run_us`,
`control.suspend_us`, `control.resume_us`, `control.fork_us`,
`control.kill_us`); a VMM where the control kernel cannot run SHALL be
listed as skipped with the reason.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs on QEMU `microvm`
- **THEN** the results contain `control.roundtrip_us` and the five operation times for guest `rhizo` on `qemu-microvm`
