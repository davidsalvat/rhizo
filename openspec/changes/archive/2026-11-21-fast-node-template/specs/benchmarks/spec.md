## ADDED Requirements

### Requirement: Node.js template frozen in its pause
The template behind `workload.node_warm_us` SHALL be frozen once all its
threads wait in the pause after the warm-up, so that every VMM measures
forks of the same state, and the pause SHALL be over before the first
fork.

#### Scenario: Forks do not wait for the pause
- **WHEN** `workload.node_warm_us` is measured
- **THEN** no fork waits for the script's 50 ms pause after `warm`, and the figure is below `workload.node_cold_us`
