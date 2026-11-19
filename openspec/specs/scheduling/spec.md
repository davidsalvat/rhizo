# scheduling Specification

## Purpose
Defines how several cells share the CPU: preemption, yielding and sleeping.

## Requirements

### Requirement: Many cells at once
The kernel SHALL keep several cells alive at the same time and run each
runnable cell in turn until it exits, is killed or sleeps.

#### Scenario: Interleaved output
- **WHEN** two cells that each print a line and yield, several times, run together
- **THEN** both complete with their full output and exit status

### Requirement: Preemption
A cell that does not make system calls SHALL be preempted when its time slice
ends, so it cannot keep other cells from running.

#### Scenario: Spinning cell
- **WHEN** a cell spins forever and another cell prints and exits
- **THEN** the second cell completes while the first is still running, and the first can then be killed

### Requirement: Yield
`sched_yield` SHALL let the next runnable cell run and return 0.

#### Scenario: Ping-pong
- **WHEN** two cells call `sched_yield` in a loop
- **THEN** they alternate and both finish

### Requirement: Sleep without CPU
A cell in `nanosleep` or `clock_nanosleep` SHALL not run until its deadline
and SHALL not consume CPU while it waits; it resumes no earlier than the
deadline.

#### Scenario: Sleep
- **WHEN** a cell sleeps 20 ms
- **THEN** the call returns 0 after at least 20 ms of monotonic time
