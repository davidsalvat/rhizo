## MODIFIED Requirements

### Requirement: TLB coherence across CPUs
No CPU SHALL use a stale translation of a cell: one that an address-space
change of the cell (unmap, protection change, copy-on-write, fork) removed
or narrowed, or one that a previous holder of the cell's tag left. Where
invalidation is per CPU (x86_64), a CPU SHALL drop the cell's tag before it
next runs the cell if the cell's translations shrank, or the cell got its
tag, since that CPU last dropped it; and when the translations of a cell
shrink while other CPUs run its code, those CPUs SHALL be interrupted and
SHALL NOT run the cell's code again until the change is complete, so that
once the system call or fault that shrank them returns, and before any
frame it freed is reused, no CPU can reach the old translation. Where
invalidation is broadcast (AArch64), no interrupt SHALL be sent for it.

#### Scenario: Copy-on-write on several CPUs
- **WHEN** forks of a cell and templates run, write and exit on 4 vCPUs
- **THEN** each sees only its own writes, as on one CPU

#### Scenario: Unmapped under a reader
- **WHEN** one thread of a cell reads a page in a loop on one CPU while another thread of the cell unmaps it on another CPU and then raises a flag
- **THEN** the reader never reads the page after it has seen the flag: its next read faults and the cell ends with a fault

### Requirement: Cells stopped before host operations
Suspend, resume, fork, freeze, kill and spawn SHALL act on cells none of
whose threads runs on any CPU: they happen between runs, after every CPU
gave back every cell, including the cells whose threads ran on several
CPUs at once.

#### Scenario: Suspend between parallel runs
- **WHEN** `counter` runs next to spinning cells on 4 vCPUs, and is suspended and resumed from its snapshot's bytes after every few slices until it exits
- **THEN** its whole output is the uninterrupted one

#### Scenario: A cell on several CPUs
- **WHEN** `parsum` computing with 4 threads on 4 vCPUs is suspended and resumed from its snapshot's bytes, or forked, between runs of a few slices, until it exits
- **THEN** the resumed cell and every child print the uninterrupted output
