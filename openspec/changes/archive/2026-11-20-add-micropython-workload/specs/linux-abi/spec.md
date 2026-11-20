# Spec Delta

## ADDED Requirements

### Requirement: Select as a sleep
The kernel SHALL implement `select` (x86_64) and `pselect6` with no
descriptors as a sleep for their timeout, and SHALL return `ENOSYS` when
descriptors are given.

#### Scenario: Timer select
- **WHEN** a cell calls `select(0, NULL, NULL, NULL, &timeout)`
- **THEN** it sleeps for the timeout and receives 0
