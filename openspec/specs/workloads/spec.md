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

### Requirement: CPython in a cell
A static build of a pinned CPython release SHALL run as a cell, unmodified,
with the standard-library modules it uses in the cell's files, and print
what the same executable prints on Linux with the same files and an empty
environment; cells forked from a template frozen after its warm-up SHALL
answer without warming up again. Where CPython's source is not given, the
checks SHALL report that they did not run.

#### Scenario: Agent request
- **WHEN** CPython runs `/app/agent_demo.py` in a cell holding the packed standard library under `/usr/lib/python3.13`
- **THEN** it prints the output recorded from Linux, nothing else, and exits with 0

#### Scenario: Warm CPython template
- **WHEN** cells are forked from CPython frozen after it printed `warm`
- **THEN** each prints only the answer and exits with 0

#### Scenario: Same behavior on Linux
- **WHEN** the differential check runs the same CPython on the same files on a Linux host of the same architecture
- **THEN** it prints the recorded output and nothing on standard error

#### Scenario: Not built
- **WHEN** `RHIZO_CPYTHON_SRC` is not set
- **THEN** the in-kernel test and the differential check say CPython was not built, and pass
