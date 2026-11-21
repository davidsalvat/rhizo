## ADDED Requirements

### Requirement: Node.js JIT metrics
Where Node.js is packed, `cargo xtask bench` SHALL report the times of
`workload.node_cold_us` and `workload.node_warm_us` measured the same way
with V8's JIT (`workload.node_jit_cold_us`, `workload.node_jit_warm_us`),
and the changes of write-or-execute pages between writing and running in
one cold run with the JIT (`workload.node_jit_flips`), from the same
kernel; where it is not packed, they SHALL be listed as skipped with the
reason.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench --workload node` runs with `RHIZO_NODE` set
- **THEN** the results contain `workload.node_jit_cold_us`, `workload.node_jit_warm_us` and `workload.node_jit_flips` for guest `rhizo`, next to the `--jitless` figures

#### Scenario: Not packed
- **WHEN** `cargo xtask bench` runs without `RHIZO_NODE`
- **THEN** `workload.node_jit_*` is listed as skipped with the reason
