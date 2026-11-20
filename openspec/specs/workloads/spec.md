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

### Requirement: A real interpreter
MicroPython SHALL run as a cell, import a module from the cell's files and
print what the same binary prints on Linux; cells forked from a template
frozen after its warm-up SHALL answer without warming up again.

#### Scenario: Python request
- **WHEN** MicroPython runs `rhizo_demo.main()` with the module in the cell's `/lib`
- **THEN** it prints the output recorded from Linux and exits with 0

#### Scenario: Warm Python template
- **WHEN** cells are forked from MicroPython frozen after it printed `warm`
- **THEN** each prints only the answer and exits with 0

### Requirement: Threaded libc programs
A static glibc program that creates threads with `pthread_create`
(default attributes, so glibc's own stacks and guard pages), synchronizes
them with mutexes and condition variables, and joins them SHALL run as a
cell and print what it prints on Linux.

#### Scenario: Producers and consumers
- **WHEN** `libc_threads` runs its producer, consumer and counter threads as a cell
- **THEN** its output and exit status equal those of the same binary on Linux
