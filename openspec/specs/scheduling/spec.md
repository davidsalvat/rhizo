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
The scheduler SHALL share the CPUs between cells, and the threads of a cell
SHALL take turns within the time its cell gets: a preempted or yielding
thread lets the cell's next runnable thread run when the cell runs again,
and a thread that blocks lets the cell's next runnable thread run at once,
in the same slice, without changing address space or tag. A thread SHALL
NOT starve the other threads of its cell.

#### Scenario: Spinning thread
- **WHEN** one thread of a cell spins forever and another thread of the same cell prints and calls `exit_group(0)`
- **THEN** the cell ends with status 0 and the output

### Requirement: Threads do not buy CPU time
Creating threads SHALL NOT give a cell more CPU time than another while
cells wait in a run queue: only CPUs that find no cell to take run more
threads of a running cell (requirement "Idle CPUs run threads of running
cells").

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

### Requirement: Waiting on descriptors
A cell whose system call must wait for a descriptor SHALL leave the run queue
until another cell's action may have made it ready or its deadline passes;
it then runs again and its call completes or waits again. The scheduler SHALL
receive the time from its caller, never read a clock.

#### Scenario: Deadline
- **WHEN** a cell waits with a deadline and nothing happens
- **THEN** it becomes runnable once the time passed to the scheduler reaches the deadline, not before

### Requirement: Cells on every CPU
A host run SHALL run its cells on every CPU online, a cell's threads on
one or more CPUs at a time, each CPU for a slice. Each CPU SHALL be
preempted by its own timer. A run SHALL end only after every CPU has given
back every cell, so that between runs no thread of any cell runs anywhere.

#### Scenario: CPU-bound cells
- **WHEN** two CPU-bound cells per CPU run on 4 vCPUs
- **THEN** they all finish, and between them they ran on all 4 CPUs

#### Scenario: Busy CPUs
- **WHEN** more cells spin for ever than there are CPUs, next to a cell that prints and exits
- **THEN** that cell completes while the spinners keep running

### Requirement: Per-CPU run queues
Each CPU SHALL have its own queue of runnable cells. A CPU SHALL take the
first cell of its own queue, and, when its queue is empty, the cell that
has waited longest in the other queues. With one CPU this SHALL be the
round robin of a single queue.

#### Scenario: Any interleaving
- **WHEN** several CPUs take, preempt and put to sleep cells in any order
- **THEN** no cell is lost or held twice, none runs before its wake time, and a CPU finds nothing only when no queue holds a cell and no sleeper is due

### Requirement: Balanced queues
A preempted or yielding cell SHALL go back to the end of its CPU's queue
unless that queue holds more cells than the shortest one; then to the end
of the shortest. A new, resumed or forked cell SHALL join the shortest
queue (the lowest-numbered CPU on ties). A sleeper whose time has come
SHALL join the queue of the CPU that finds it.

#### Scenario: Nobody starves
- **WHEN** in every round each CPU takes a cell and gives it back, in any order
- **THEN** every cell runs at least once in any run of as many rounds as there are cells, and the queues stay balanced

### Requirement: Stepping on one CPU
`Host::step` SHALL run one slice on the calling CPU alone: the runnable cell
that has waited longest in any queue, or, when every cell sleeps, a wait
for the first to wake. Step counts SHALL therefore not depend on the number
of CPUs.

#### Scenario: Thread count does not buy time on several CPUs
- **WHEN** the in-kernel tests that count scheduling steps run on 4 vCPUs
- **THEN** they observe the same step counts as on one CPU

### Requirement: Idle CPUs run threads of running cells
A CPU that finds no cell in any run queue SHALL join a running cell that
has a runnable thread no CPU runs (the cell started running first, first)
and run that thread; the cell's threads SHALL then take turns among the
CPUs that run the cell, by thread identifier. A cell with one thread,
`Host::step` and a kernel with one CPU SHALL run every cell on one CPU at
a time.

#### Scenario: Four threads, four CPUs
- **WHEN** a cell with four CPU-bound threads runs alone on 4 vCPUs
- **THEN** the kernel records that its threads ran on at least two CPUs at the same time, and `cell.thread_scaling_pct` measures how much sooner it finishes than with one thread

### Requirement: Leaving a running cell
A CPU SHALL leave a cell it runs with other CPUs when its thread blocks and
no other thread of the cell is left for it, when its thread yields, and at
the end of its slice. The last CPU to leave SHALL give the cell back to
the run queues as one cell: runnable, waiting or asleep, as its threads
say, or ended.

#### Scenario: Any interleaving
- **WHEN** several CPUs start, join and leave running cells in any order
- **THEN** a CPU runs at most one cell, joins only a cell that offers a thread, and exactly the last CPU to leave a cell gets it back
