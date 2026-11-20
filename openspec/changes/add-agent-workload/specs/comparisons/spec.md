# Spec Delta

## ADDED Requirements

### Requirement: The agent as a Linux process in the same VM
`cargo xtask bench` SHALL run the CPython agent as a Linux process inside
the Firecracker guest of the CPython comparison, reaching the host's fake
model through the guest's network card and forking its branches with
`fork`, and report its turn and its fork until every branch runs; where it
cannot, it SHALL list them as skipped with the reason.

#### Scenario: Measured in the guest
- **WHEN** `cargo xtask bench` runs on a host with KVM, password-less `sudo` and CPython built
- **THEN** the results contain `workload.agent_cpython_turn_us` and `workload.agent_fork_us` for guest `linux-process` on `firecracker`

#### Scenario: No tap device
- **WHEN** the tap device cannot be made
- **THEN** the agent comparison is listed as skipped with the reason, and the CPython figures are still reported
