# Spec Delta

## ADDED Requirements

### Requirement: Forks carry every thread
Forking a cell and spawning from a template SHALL copy every thread of the
cell (registers, FP/SIMD state, thread pointer, identifier, clear-tid
address and what it waits for). Unlike Linux `fork`, which copies only the
calling thread, the host forks a cell from outside, so no thread is the
caller, and dropping threads would leave the locks they hold locked.

#### Scenario: Threaded children
- **WHEN** a cell whose worker threads wait on a condition variable is forked, and each copy then signals its workers
- **THEN** in every copy all workers run, and each copy prints the same output

### Requirement: Templates keep no time
In a cell spawned from a template, the sleeps of the template's threads
SHALL be over and their futex waits with a timeout SHALL have timed out;
futex waits without a timeout SHALL still wait.

#### Scenario: Template of a threaded program
- **WHEN** cells are spawned from a template of a multi-threaded program frozen after its warm-up
- **THEN** each runs all its threads to the same answer as a cold run
