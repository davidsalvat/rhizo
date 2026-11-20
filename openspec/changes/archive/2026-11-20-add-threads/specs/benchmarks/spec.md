# Spec Delta

## ADDED Requirements

### Requirement: Thread metrics
`cargo xtask bench` SHALL report `cell.thread_create_us`, the time for a
thread of a cell to create a thread that exits at once and to join it, and
`cell.futex_wake_ns`, the time for one thread to wake another blocked on a
futex until that one runs (half of a ping-pong round trip), for every VMM
that measures cells.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs on a VMM where the counter frequency is known
- **THEN** the results contain `cell.thread_create_us` and `cell.futex_wake_ns`
