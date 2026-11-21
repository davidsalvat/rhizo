## MODIFIED Requirements

### Requirement: Start-up calls of distribution programs
The kernel SHALL implement `statx` (the fields `newfstatat` gives),
`sysinfo`, `sched_getaffinity` (one CPU) and `fadvise64` (accepted,
nothing to do), as coreutils and CPython call them at start-up. `sysinfo`
SHALL describe the calling cell only: uptime, the cell's memory quota (at
most the memory cells share) as total memory, what the cell's own use
leaves of it as free memory, its threads as processes, no swap and no
load.

#### Scenario: Number of CPUs
- **WHEN** a cell asks `sched_getaffinity` for its CPUs with a buffer of 128 bytes
- **THEN** it receives 8 and a mask with CPU 0 only

#### Scenario: Memory of the cell
- **WHEN** a cell with a 64 MiB memory quota and three threads calls `sysinfo`
- **THEN** it reads 64 MiB of total memory, its quota less what it uses as free memory, and 3 processes
