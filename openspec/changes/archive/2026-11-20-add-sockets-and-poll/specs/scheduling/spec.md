# Spec Delta

## ADDED Requirements

### Requirement: Waiting on descriptors
A cell whose system call must wait for a descriptor SHALL leave the run queue
until another cell's action may have made it ready or its deadline passes;
it then runs again and its call completes or waits again. The scheduler SHALL
receive the time from its caller, never read a clock.

#### Scenario: Deadline
- **WHEN** a cell waits with a deadline and nothing happens
- **THEN** it becomes runnable once the time passed to the scheduler reaches the deadline, not before
