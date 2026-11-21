## MODIFIED Requirements

### Requirement: Waiting cells use no CPU
A cell waiting on descriptors SHALL not run until one of them may be ready
or its timeout ends; other cells run meanwhile. Each socket SHALL keep a
wait queue of the cells whose waiting calls looked at it, and a change to
the socket (bytes or a datagram queued, room for a writer whose peer's
queue was full, a connection to accept, a shutdown, a close, a reset, a
change of a connection through the network card) SHALL wake only the
waiters on its queue. The queues SHALL NOT be part of a cell's snapshot: a
resumed or forked cell makes its waiting calls again.

#### Scenario: Server waits for its client
- **WHEN** a server cell blocks in `accept` and a client cell is spawned later
- **THEN** the server does not run until the client connects, then both finish

#### Scenario: A change wakes the waiters of its socket only
- **WHEN** a waiter blocked after looking at one connection and bytes are sent on another connection
- **THEN** the waiter is not woken; bytes sent on its own connection wake it once

#### Scenario: A writer waits for room
- **WHEN** a writer blocked on a full connection and the reader reads from it
- **THEN** the writer is woken; a later read from a queue that is no longer full wakes nobody

#### Scenario: A frame from the wire
- **WHEN** a waiter blocked on a connection through the network card and the answer arrives
- **THEN** the waiter is woken, and a waiter on a loopback socket is not
