## MODIFIED Requirements

### Requirement: Threads share their cell's time
The scheduler SHALL share the CPUs between cells, and the threads of a cell
SHALL take turns within the time its cell gets: a preempted or yielding
thread lets the cell's next runnable thread run when the cell runs again,
and a thread that blocks lets the cell's next runnable thread run at once,
in the same slice, without changing address space or tag. A thread SHALL
NOT starve the other threads of its cell, and creating threads SHALL NOT
give a cell more CPU time than another while cells wait in a run queue:
only CPUs that find no cell to take run more threads of a running cell
(requirement "Idle CPUs run threads of running cells").

#### Scenario: Spinning thread
- **WHEN** one thread of a cell spins forever and another thread of the same cell prints and calls `exit_group(0)`
- **THEN** the cell ends with status 0 and the output

#### Scenario: Thread count does not buy time
- **WHEN** a cell that prints and yields runs next to a cell with four spinning threads, and then next to a cell with one spinning thread
- **THEN** it completes after the same number of scheduling steps both times

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

## ADDED Requirements

### Requirement: Idle CPUs run threads of running cells
A CPU that finds no cell in any run queue SHALL join a running cell that
has a runnable thread no CPU runs (the cell started running first, first)
and run that thread; the cell's threads SHALL then take turns among the
CPUs that run the cell, by thread identifier. A cell with one thread,
`Host::step` and a kernel with one CPU SHALL run every cell on one CPU at
a time.

#### Scenario: Four threads, four CPUs
- **WHEN** a cell with four CPU-bound threads runs alone on 4 vCPUs
- **THEN** its threads run on several CPUs at once and it finishes in less time than with one thread

### Requirement: Leaving a running cell
A CPU SHALL leave a cell it runs with other CPUs when its thread blocks and
no other thread of the cell is left for it, when its thread yields, and at
the end of its slice. The last CPU to leave SHALL give the cell back to
the run queues as one cell: runnable, waiting or asleep, as its threads
say, or ended.

#### Scenario: Any interleaving
- **WHEN** several CPUs start, join and leave running cells in any order
- **THEN** a CPU runs at most one cell, joins only a cell that offers a thread, and exactly the last CPU to leave a cell gets it back
