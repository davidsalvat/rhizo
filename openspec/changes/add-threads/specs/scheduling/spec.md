# Spec Delta

## ADDED Requirements

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
