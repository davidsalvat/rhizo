# Spec Delta

## ADDED Requirements

### Requirement: Threaded libc programs
A static glibc program that creates threads with `pthread_create`
(default attributes, so glibc's own stacks and guard pages), synchronizes
them with mutexes and condition variables, and joins them SHALL run as a
cell and print what it prints on Linux.

#### Scenario: Producers and consumers
- **WHEN** `libc_threads` runs its producer, consumer and counter threads as a cell
- **THEN** its output and exit status equal those of the same binary on Linux
