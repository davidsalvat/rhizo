# Spec Delta

## ADDED Requirements

### Requirement: Module metrics
`cargo xtask bench` SHALL report `module.add_us` (from an add request to
the module's first reply, for `python` and `numpy`), `module.extra_bytes`
(memory of each extra idle module child), `module.file_bytes` (a
`python+numpy` delta file against a full one), `chat.static_bytes` and
`chat.static_turn_us` (the static chat agent against the CPython one).

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs with CPython built
- **THEN** the results contain the five metrics, and no gated metric of system calls, switches, forks or the network regressed
