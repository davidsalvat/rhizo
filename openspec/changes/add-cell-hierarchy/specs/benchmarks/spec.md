# Spec Delta

## ADDED Requirements

### Requirement: Containment metrics
`cargo xtask bench` SHALL report `net.egress_check_insns`, the
instructions of one egress check against 16 rules, `net.tcp_rtt_us` with
such a policy alongside the figure without, and `covert.cpu_bps`, the
measured bandwidth of a CPU-contention channel between two domains,
reported and not gated.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs a `bench-cell` kernel
- **THEN** the results contain `net.egress_check_insns` and `covert.cpu_bps`, and the gated metrics of system calls, switches, forks and loopback are within their thresholds

### Requirement: Channel metrics
`cargo xtask bench` SHALL report `cell.channel_rtt_us` (two cells
exchanging a 64-byte message over a channel, per round trip, against
`cell.loopback_rtt_us`), `cell.handle_pass_us` (a handle sent and
received) and `cell.revoke_us` (revoking a grant with 100 derived), as
robust cell figures.

#### Scenario: Channel faster than loopback
- **WHEN** the channel metrics are measured
- **THEN** `cell.channel_rtt_us` is reported and is not above `cell.loopback_rtt_us`

### Requirement: View and child metrics
`cargo xtask bench` SHALL report `cell.view_grant_us` (a snapshot view of
a 16 MiB tree), `cell.view_read_us` (1 MiB read through a live view,
against the cell's own files), `cell.grant_us` (a grant with its exposure
check in a domain of 64 cells) and `cell.child_spawn_us` (a cell forking a
child through its kernel endpoint, to the child's exit).

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs a `bench-cell` kernel
- **THEN** the results contain the four metrics
