# Spec Delta

## MODIFIED Requirements

### Requirement: Network metrics
`cargo xtask bench` SHALL report `cell.pipe_rtt_ns`, the time for a cell to
write one byte into a pipe and read it back, `cell.loopback_rtt_us`, the
round-trip time of a 64-byte message over TCP on 127.0.0.1 between two
threads of one cell (cells no longer share a loopback network), and
`cell.tcp_resume_us`, the time to resume such a cell suspended with an
established loopback connection until it has made one more 64-byte round
trip over it and exited, as robust cell figures.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs a `bench-cell` kernel where the counter frequency is known
- **THEN** the results contain `cell.pipe_rtt_ns`, `cell.loopback_rtt_us` and `cell.tcp_resume_us`

## ADDED Requirements

### Requirement: Containment metrics
`cargo xtask bench` SHALL report `net.egress_check_insns` (one egress
check against 16 rules), `net.tcp_rtt_us` with such a policy next to the
figure without, and `covert.cpu_bps` (a CPU-contention channel between
two domains) with and without exclusive CPU sets, reported and not gated.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs a `bench-cell` kernel with 4 vCPUs
- **THEN** the results contain `net.egress_check_insns` and both `covert.cpu_bps` figures, and the gated metrics of system calls, switches, forks and loopback are within their thresholds

### Requirement: Fairness metrics
`cargo xtask bench` SHALL report `sched.domain_share_pct`, the CPU share
of a domain of one spinning cell next to a domain of 16 on one CPU with
equal weights, reported and not gated, while `cell.switch_ns`,
`cell.parallel_us` and `cell.scaling_pct` with one domain stay within
their thresholds.

#### Scenario: Metric present
- **WHEN** `cargo xtask bench` runs a `bench-cell` kernel
- **THEN** the results contain `sched.domain_share_pct`, and the gated switch and parallel metrics pass

### Requirement: Channel metrics
`cargo xtask bench` SHALL report `cell.channel_rtt_us` (two cells
exchanging a 64-byte message over a channel, per round trip),
`cell.handle_pass_us` (a handle sent and received) and `cell.revoke_us`
(revoking a grant with 100 derived), as robust cell figures.

#### Scenario: Channel between cells
- **WHEN** the channel metrics are measured
- **THEN** `cell.channel_rtt_us` is reported and is not above the last two-cell `cell.loopback_rtt_us` of the baseline before this change

### Requirement: View and child metrics
`cargo xtask bench` SHALL report `cell.view_grant_us` (a snapshot view of
a 16 MiB tree), `cell.view_read_us` (1 MiB read through a live view,
against the cell's own files), `cell.grant_us` (a grant with its exposure
check in a domain of 64 cells) and `cell.child_spawn_us` (a cell forking a
child through its kernel endpoint, to the child's exit).

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs a `bench-cell` kernel
- **THEN** the results contain the four metrics
