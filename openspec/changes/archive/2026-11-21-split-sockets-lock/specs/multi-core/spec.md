## ADDED Requirements

### Requirement: System calls without sockets do not take turns
A system call that uses no socket SHALL NOT wait for the kernel's sockets
lock. A system call that uses sockets SHALL hold that lock from its first
use of a socket until it has joined the wait queues of the sockets it
looked at. The scheduler SHALL take the lock only when a change woke
waiters that were not woken yet.

#### Scenario: Pipes, counters and epoll
- **WHEN** a cell writes and reads a pipe and an event counter, and waits on an `epoll` instance holding only the counter
- **THEN** none of these calls reaches the sockets, and a call creating a socket does

#### Scenario: Cells making system calls on every CPU
- **WHEN** four cells per CPU write and read their own pipe 20,000 times each, on 4 CPUs at once
- **THEN** they finish and `cell.syscall_scaling_pct` is reported
