# Spec Delta

## MODIFIED Requirements

### Requirement: Start-up calls of distribution programs
The kernel SHALL implement `statx` (the fields `newfstatat` gives),
`sysinfo`, `sched_getaffinity` (one CPU) and `fadvise64` (accepted,
nothing to do), as coreutils and CPython call them at start-up. `sysinfo`
SHALL describe the calling cell only, whatever other domains do: uptime,
the cell's own memory limit (at most the memory cells share) as total
memory, what the cell's own use leaves of it as free memory, its threads
as processes, no swap and no load.

#### Scenario: Number of CPUs
- **WHEN** a cell asks `sched_getaffinity` for its CPUs with a buffer of 128 bytes
- **THEN** it receives 8 and a mask with CPU 0 only

#### Scenario: Memory of the cell
- **WHEN** a cell with a 64 MiB memory quota and three threads calls `sysinfo`
- **THEN** it reads 64 MiB of total memory, its quota less what it uses as free memory, and 3 processes

#### Scenario: Memory of a limited cell
- **WHEN** a cell with a 64 MiB limit that has taken 10 MiB calls `sysinfo`, while another domain takes and frees memory
- **THEN** it reads 64 MiB total and 54 MiB free, whatever the other domain does

## ADDED Requirements

### Requirement: Identifiers per domain
`getpid` SHALL return the cell's identifier within its domain, counted
from 1 for the domain's root in the order its cells are made, and thread
identifiers SHALL derive from it, so that no identifier tells anything of
other domains.

#### Scenario: Busy neighbour
- **WHEN** another domain makes and ends 500 cells between the start of two roots
- **THEN** both roots read 1 from `getpid`

### Requirement: Copies are told apart
`madvise` SHALL accept `MADV_WIPEONFORK` and `MADV_KEEPONFORK` on
private anonymous memory: in every copy of a cell (fork, template copy,
load of a snapshot file, resume) the wiped ranges SHALL read as zero,
and the source SHALL keep its contents. A copy's `getpid` SHALL differ
from its source's, so libraries that detect a fork by it reseed.

#### Scenario: Wiped page
- **WHEN** a cell marks a page `MADV_WIPEONFORK`, writes 1 in it, and is forked, copied from a template of it and resumed from its snapshot
- **THEN** each copy and the resumed cell read 0, and the source reads 1

#### Scenario: Copy as a root
- **WHEN** a root whose `getpid` is 1 is frozen as a published template and copied as a new root
- **THEN** the copy's `getpid` is not 1
