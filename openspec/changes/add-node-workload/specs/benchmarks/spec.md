# Spec Delta

## ADDED Requirements

### Requirement: Node.js workload metrics
Where Node.js is packed, `cargo xtask bench` SHALL report the time for a
Node.js cell (`--jitless`) to answer one agent request started cold from
its files (`workload.node_cold_us`: interpreter, start-up, warm-up, answer,
exit) and forked from a template frozen after the warm-up
(`workload.node_warm_us`), for every VMM that measures cells, from a
kernel and VM of their own; `--workload node` SHALL measure them alone.
Where it is not packed, they SHALL be listed as skipped with the reason.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs with `RHIZO_NODE` set
- **THEN** the results contain `workload.node_cold_us` and `workload.node_warm_us` for guest `rhizo`

#### Scenario: Not packed
- **WHEN** `cargo xtask bench` runs without `RHIZO_NODE`
- **THEN** `workload.node_*` is listed as skipped with the reason
