# Spec Delta

## MODIFIED Requirements

### Requirement: Start-up calls of distribution programs
The kernel SHALL implement `statx` (the fields `newfstatat` gives),
`sysinfo` (uptime, and as total and free memory the cell's own memory
limit and what is left of it, or the kernel's for an unlimited cell; no
swap, one process), `sched_getaffinity` (one CPU) and `fadvise64`
(accepted, nothing to do), as coreutils and CPython call them at start-up.

#### Scenario: Number of CPUs
- **WHEN** a cell asks `sched_getaffinity` for its CPUs with a buffer of 128 bytes
- **THEN** it receives 8 and a mask with CPU 0 only

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
