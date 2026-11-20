## ADDED Requirements

### Requirement: Threads of a cell in parallel
With several CPUs online, the threads of a cell SHALL be able to run on
different CPUs at the same time, sharing the cell's memory, files and
mappings exactly as they do on one CPU. The system calls of one cell, and
the faults that change its page tables, SHALL take effect one at a time,
each as if the cell had one CPU. A cell's exit (`exit_group`, the last
`exit`, a fault) SHALL stop its threads on every CPU before its memory is
freed.

#### Scenario: Parallel sum
- **WHEN** `parsum` sums with 4 threads, under a mutex and a condition variable, on 4 vCPUs
- **THEN** it prints the sum it prints with 1 thread and on Linux, and its threads ran on at least two CPUs at the same time

#### Scenario: Exit while siblings run elsewhere
- **WHEN** one thread of a cell calls `exit_group(0)` while another spins on another CPU
- **THEN** the cell ends with status 0 and every frame it held is returned

### Requirement: Futex wake-ups across CPUs
A futex wake, a `clone` or a thread exit that makes a thread of a running
cell runnable SHALL let an idle CPU run that thread at once (an
inter-processor interrupt wakes it); a CPU waiting for work SHALL also wake
when a deadline of a waiting thread of a running cell comes.

#### Scenario: Ping-pong between CPUs
- **WHEN** two threads of a cell hand a futex word to each other many times on 4 vCPUs
- **THEN** every wait returns once the other thread has woken it, and the cell ends with the output it has on one CPU
