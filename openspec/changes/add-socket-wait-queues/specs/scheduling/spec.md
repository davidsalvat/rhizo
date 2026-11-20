## MODIFIED Requirements

### Requirement: Waiting on descriptors
A cell whose system call must wait for a descriptor SHALL leave the run queue
until a socket the call looked at changes, a thread of the same cell changes
one of its pipes, event counters or `epoll` instances, or its deadline
passes; it then runs again and its call completes or waits again. A change
to a socket SHALL NOT make runnable a cell none of whose calls waits on that
socket. A change made while the cell is still being filed as waiting SHALL
NOT be lost. The scheduler SHALL receive the time from its caller, never
read a clock.

#### Scenario: Deadline
- **WHEN** a cell waits with a deadline and nothing happens
- **THEN** it becomes runnable once the time passed to the scheduler reaches the deadline, not before

#### Scenario: Only the waiters of a socket wake
- **WHEN** a server cell waits in `accept` on one port while a client and a server cell exchange 50 messages on another port
- **THEN** the idle server never becomes runnable, and no cell woken by a socket change finds nothing and waits again

#### Scenario: A thread waits on a pipe of its cell
- **WHEN** one thread of a cell waits to read an empty pipe and another thread of the cell writes to it
- **THEN** the waiting thread becomes runnable and its read completes

#### Scenario: Waking one waiter
- **WHEN** several cells wait and one of them is woken by its key
- **THEN** only that one becomes runnable, and the others keep waiting until their own wake-up or deadline
