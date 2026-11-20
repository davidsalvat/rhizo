# Spec Delta

## ADDED Requirements

### Requirement: Network metrics
`cargo xtask bench` SHALL report `cell.pipe_rtt_ns`, the time for a cell to
write one byte into a pipe and read it back, and `cell.loopback_rtt_us`, the
round-trip time of a 64-byte message between two cells over TCP on
127.0.0.1, as robust cell figures.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs a `bench-cell` kernel where the counter frequency is known
- **THEN** the results contain `cell.pipe_rtt_ns` and `cell.loopback_rtt_us`
