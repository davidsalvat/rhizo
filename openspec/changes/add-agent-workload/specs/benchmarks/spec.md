# Spec Delta

## ADDED Requirements

### Requirement: Agent metrics
`cargo xtask bench` SHALL report, where the CPython agent can run, one of
its turns as the agent times it, the first left out
(`workload.agent_turn_us`), a suspend plus eager resume between two turns
(`workload.agent_suspend_resume_us`) and a fork from its decision point
until its three branches run (`workload.agent_fork_us`); elsewhere it SHALL
list them as skipped with the reason.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs on Firecracker with KVM and CPython built
- **THEN** the results contain `workload.agent_turn_us`, `workload.agent_suspend_resume_us` and `workload.agent_fork_us` for guest `rhizo` on `firecracker`

#### Scenario: No model server
- **WHEN** the VMM gives the kernel no network card
- **THEN** the agent metrics are listed as skipped with the reason
