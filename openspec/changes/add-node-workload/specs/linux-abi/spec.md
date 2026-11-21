# Spec Delta

## ADDED Requirements

### Requirement: Time of day and clock resolution
The kernel SHALL implement `gettimeofday` (the realtime clock in seconds
and microseconds, an empty time zone; either pointer may be null) and
`clock_getres` (one nanosecond for every clock `clock_gettime` reads,
`EINVAL` for another), as libuv calls them; a bad pointer SHALL return
`EFAULT`.

#### Scenario: Time of day
- **WHEN** a cell calls `gettimeofday` with a buffer for the time and one for the time zone
- **THEN** it receives 0, the realtime clock's seconds and microseconds, and a zeroed time zone

#### Scenario: Resolution of an unknown clock
- **WHEN** a cell calls `clock_getres` for a clock `clock_gettime` does not read
- **THEN** it receives `-EINVAL`

### Requirement: Capabilities and thread names
`capget` SHALL report that a cell holds no capabilities: versions 1 to 3 of
the header get empty sets, another version is answered with version 3 in
the header and `EINVAL`. `prctl` SHALL accept `PR_SET_NAME` (the name is
not kept) and `PR_GET_NAME` (an empty name), and return `EINVAL` for any
other option.

#### Scenario: Capabilities of a cell
- **WHEN** a cell calls `capget` with a version 3 header
- **THEN** it receives 0 and empty effective, permitted and inheritable sets

#### Scenario: A thread names itself
- **WHEN** a cell calls `prctl` with `PR_SET_NAME`
- **THEN** it receives 0

### Requirement: Scheduling policy of threads
`sched_getparam` and `sched_getscheduler` SHALL report the normal policy
and priority 0 for the caller (0) or one of its threads, `ESRCH` for any
other thread and `EINVAL` for a negative one.

#### Scenario: Policy of another thread
- **WHEN** a cell calls `sched_getscheduler` for a thread that is not one of its own
- **THEN** it receives `-ESRCH`

### Requirement: Declined calls
`io_uring_setup` SHALL return `-ENOSYS` without being reported as an
unsupported call, as `rseq` does: libraries that try it (libuv) fall back to
calls cells have.

#### Scenario: libuv probes io_uring
- **WHEN** a cell calls `io_uring_setup`
- **THEN** it receives `-ENOSYS` and the console shows nothing
