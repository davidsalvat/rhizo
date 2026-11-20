# Spec Delta

## ADDED Requirements

### Requirement: The host wakes a sleeping cell
The host SHALL be able to end the sleeps of a live cell's threads and time
out their timed waits, as an event from outside the cell would; the cell
is then runnable at once. Waking a cell that does not exist SHALL report
that it does not.

#### Scenario: Woken worker
- **WHEN** the host wakes a `worker` sleeping for an hour after `ready`
- **THEN** it runs its one second of operations and exits with 0

#### Scenario: No such cell
- **WHEN** the host wakes a cell identifier that is not live
- **THEN** the call reports that no cell was woken
