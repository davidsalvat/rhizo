## MODIFIED Requirements

### Requirement: Cells stopped before host operations
Suspend, resume, fork, freeze, kill and spawn SHALL act on cells none of
whose threads runs on any CPU, including cells whose threads ran on
several CPUs at once. Made between runs, they SHALL find every cell given
back. Made from a run's stop condition, while the other CPUs run cells,
an operation on a cell SHALL stop only the CPUs running that cell, and
only until they have given it back; a cell that ends meanwhile SHALL be
reported as ended, and no other cell SHALL be stopped.

#### Scenario: Suspend between parallel runs
- **WHEN** `counter` runs next to spinning cells on 4 vCPUs, and is suspended and resumed from its snapshot's bytes after every few slices until it exits
- **THEN** its whole output is the uninterrupted one

#### Scenario: A cell on several CPUs
- **WHEN** `parsum` computing with 4 threads on 4 vCPUs is suspended and resumed from its snapshot's bytes, or forked, between runs of a few slices, until it exits
- **THEN** the resumed cell and every child print the uninterrupted output

#### Scenario: Operations during a run
- **WHEN** on 4 vCPUs, from the stop condition of one run, the host suspends and resumes `counter` from its snapshot's bytes after every slice of the boot CPU, suspends, resumes and forks `parsum` computing with 4 threads, and kills and replaces a spinning cell, until `counter`, `parsum` and the fork end
- **THEN** each prints the uninterrupted output and no cell is lost

## ADDED Requirements

### Requirement: Host operations do not pause other cells
A host operation made from a run's stop condition SHALL NOT stop a cell it
does not act on: on a machine with CPUs to spare, such a cell SHALL keep
running through the operation.

#### Scenario: A cell counting through forks and snapshots
- **WHEN** on 4 vCPUs `progress` counts while, in five batches of at least 50 ms each, the host forks a spinning cell, kills the child, and suspends and resumes the spinner, from the run's stop condition
- **THEN** the count read from `progress`'s memory, without stopping it, grows during every batch
