# Spec Delta

## ADDED Requirements

### Requirement: Cells on every CPU
A host run SHALL run its cells on every CPU online, each cell on one CPU at
a time for a slice, its threads sharing the cell's slice. Each CPU SHALL be
preempted by its own timer. A run SHALL end only after every CPU has given
its cell back, so that between runs no cell runs anywhere.

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
