## MODIFIED Requirements

### Requirement: Cells on every CPU
A host run SHALL run its cells on every CPU online, a cell's threads on
one or more CPUs at a time, each CPU for a slice. Each CPU SHALL be
preempted by its own timer. A run SHALL end only after every CPU has given
back every cell, so that between runs no thread of any cell runs anywhere.
Cells that a host operation made runnable during a run SHALL be taken by
an idle CPU, if one is idle, without waiting for the boot CPU.

#### Scenario: CPU-bound cells
- **WHEN** two CPU-bound cells per CPU run on 4 vCPUs
- **THEN** they all finish, and between them they ran on all 4 CPUs

#### Scenario: Busy CPUs
- **WHEN** more cells spin for ever than there are CPUs, next to a cell that prints and exits
- **THEN** that cell completes while the spinners keep running

## ADDED Requirements

### Requirement: Cells found by identifier
The scheduler SHALL find, take out and change a cell by its identifier,
wherever it is (a run queue, the sleepers, the waiting set), without
reading the other cells it holds.

#### Scenario: Any place
- **WHEN** entries are runnable, sleeping and waiting on several CPUs' queues, and one is changed, found again and taken out by its key
- **THEN** the right entry comes out, with its wake time if it slept, and a key no entry has finds nothing
