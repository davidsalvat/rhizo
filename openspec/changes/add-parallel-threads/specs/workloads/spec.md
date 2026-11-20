## ADDED Requirements

### Requirement: Parallel threaded programs
A static glibc program that splits a computation between `pthread_create`
threads, which take work and add their results under a mutex and signal a
condition variable the first thread waits on, SHALL run as a cell, print
what it prints on Linux, and finish sooner with several threads than with
one when the CPUs are idle.

#### Scenario: Parallel sum against Linux
- **WHEN** `parsum` runs as a cell
- **THEN** its output and exit status equal those of the same binary on Linux
