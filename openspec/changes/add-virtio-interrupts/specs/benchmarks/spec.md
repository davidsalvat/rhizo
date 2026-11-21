## ADDED Requirements

### Requirement: Wake-ups per agent turn
The laptop workload SHALL report `laptop.turn_wakeups`: the guest CPUs'
returns from waiting for an interrupt during the agent turns, per turn.

#### Scenario: Metric present
- **WHEN** `cargo xtask bench --workload laptop` runs with a fake model server
- **THEN** the results contain `laptop.turn_wakeups`
