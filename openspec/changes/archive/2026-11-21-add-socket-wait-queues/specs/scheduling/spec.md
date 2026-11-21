## MODIFIED Requirements

### Requirement: Waiting on descriptors
A cell whose system call must wait for a descriptor SHALL leave the run queue
until a socket the call looked at changes or its deadline passes; it then
runs again and its call completes or waits again. A change to a socket SHALL
NOT make runnable a cell none of whose calls waits on that socket. The
scheduler SHALL receive the time from its caller, never read a clock.

#### Scenario: Deadline
- **WHEN** a cell waits with a deadline and nothing happens
- **THEN** it becomes runnable once the time passed to the scheduler reaches the deadline, not before

#### Scenario: Only the waiters of a socket wake
- **WHEN** a server cell waits in `accept` on one port while a client and a server cell exchange 50 messages on another port
- **THEN** the idle server never becomes runnable, and no cell woken by a socket change finds nothing and waits again

#### Scenario: Waking one waiter
- **WHEN** several cells wait and one of them is woken by its key
- **THEN** only that one becomes runnable, and the others keep waiting until their own wake-up or deadline

## ADDED Requirements

### Requirement: Waits on a cell's own descriptors
A thread waiting on descriptors SHALL also run again when another thread of
the same cell changes one of the cell's pipes, event counters or `epoll`
instances.

#### Scenario: A thread waits on a pipe of its cell
- **WHEN** one thread of a cell waits to read an empty pipe and another thread of the cell writes to it
- **THEN** the waiting thread becomes runnable and its read completes

### Requirement: No lost socket change
A socket change made while a cell is still being filed as waiting (by
another CPU, after the cell's call found nothing ready) SHALL NOT be lost:
the cell SHALL look again once it is filed or next taken.

#### Scenario: A change before the filing
- **WHEN** a cell's call finds nothing ready and, before the cell is filed as waiting, another CPU queues bytes on that socket
- **THEN** the cell runs again and its call completes

### Requirement: Host wake-ups are not lost
A wake from the host (`Host::wake`) SHALL end the sleeps and time out the
timed waits of the cell's threads; when none of them waits with a
deadline yet, the next wait with a deadline of one of its threads SHALL end
at once.

#### Scenario: Woken on the way to its sleep
- **WHEN** the host wakes a cell that printed `ready` and yielded, and that will then sleep for an hour
- **THEN** its sleep ends at once and it exits within two scheduling steps
