# Spec Delta

## ADDED Requirements

### Requirement: Scheduling calls
The kernel SHALL implement `sched_yield`, `nanosleep` and `clock_nanosleep`
(relative sleeps on `CLOCK_MONOTONIC` and `CLOCK_REALTIME`, and absolute
`TIMER_ABSTIME` sleeps) with Linux semantics; a bad pointer returns `-EFAULT`
and an invalid time `-EINVAL`.

#### Scenario: Invalid sleep
- **WHEN** a cell calls `nanosleep` with nanoseconds outside 0–999,999,999
- **THEN** it receives `-EINVAL`
