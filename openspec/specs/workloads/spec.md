# workloads Specification

## Purpose
Defines running real Linux programs, built with a real libc, as cells and
holding them to the behavior they have on Linux.

## Requirements

### Requirement: Real libc programs
Static programs built with the system's C library SHALL run as cells
unchanged and produce the same output and exit status as on Linux.

#### Scenario: glibc program
- **WHEN** a static glibc program that formats numbers, sorts, allocates large and small blocks and uses floating point runs as a cell
- **THEN** its output and exit status equal those of the same binary on Linux

### Requirement: Warm templates of real programs
A real program frozen after its warm-up SHALL serve each new request as a
fork that does not repeat the warm-up.

#### Scenario: Warm workload
- **WHEN** cells are spawned from a template of `libc_warm` frozen after it printed `warm`
- **THEN** each prints only its answer, the same as a cold run's answer
