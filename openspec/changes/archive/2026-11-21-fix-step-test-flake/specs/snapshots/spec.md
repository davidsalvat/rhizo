# Spec Delta

## MODIFIED Requirements

### Requirement: Resume
The kernel SHALL resume a snapshot as a new live cell that continues from the
exact point where it was suspended.

#### Scenario: Equivalence
- **WHEN** a cell that ends every step of its work with `sched_yield`, and sleeps in some of them, is suspended and resumed after every scheduling step until it exits
- **THEN** its complete output and exit status are identical to an uninterrupted run, and it was suspended at least once per step
