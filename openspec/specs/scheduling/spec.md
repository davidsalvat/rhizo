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

### Requirement: Threads share their cell's time
The scheduler SHALL share the CPU between cells, and the threads of a cell
SHALL take turns within the time its cell gets: a preempted or yielding
thread lets the cell's next runnable thread run when the cell runs again,
and a thread that blocks lets the cell's next runnable thread run at once,
in the same slice, without changing address space or tag. A thread SHALL
NOT starve the other threads of its cell, and creating threads SHALL NOT
give a cell more CPU than another.

#### Scenario: Spinning thread
- **WHEN** one thread of a cell spins forever and another thread of the same cell prints and calls `exit_group(0)`
- **THEN** the cell ends with status 0 and the output

#### Scenario: Thread count does not buy time
- **WHEN** a cell that prints and yields runs next to a cell with four spinning threads, and then next to a cell with one spinning thread
- **THEN** it completes after the same number of scheduling steps both times

### Requirement: Blocked threads use no CPU
A thread waiting on a futex or sleeping SHALL NOT run until it is woken or
its time comes. A cell whose threads all wait SHALL NOT run until the
earliest timeout among them; when none of them has a timeout, the cell SHALL
never run again on its own (as on Linux, it is deadlocked) and can still be
killed, suspended or forked.

#### Scenario: Waiting threads
- **WHEN** a cell's threads all wait on futexes without timeout while another cell runs
- **THEN** the other cell completes, the waiting cell stays live without running, and it can be killed
