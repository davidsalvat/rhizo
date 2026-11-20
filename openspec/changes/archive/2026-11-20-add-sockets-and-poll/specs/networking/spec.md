# Spec Delta

## ADDED Requirements

### Requirement: Loopback network between cells
Cells of one kernel SHALL share a loopback network (127.0.0.0/8): a TCP
connection or a UDP datagram from one cell SHALL reach a socket of another
cell listening or bound at the destination address and port, in order and
without loss.

#### Scenario: Echo between two cells
- **WHEN** one cell listens on a loopback port and another connects, sends messages and reads the replies
- **THEN** each reply equals its message and both cells exit with status 0

### Requirement: Loopback addresses only
Until the kernel has a network card, binding to an address other than
127.0.0.0/8 or `INADDR_ANY` SHALL fail with `EADDRNOTAVAIL`, and connecting
or sending to one with `ENETUNREACH`. Port 0 SHALL pick a free port in
32768–60999.

#### Scenario: Outside address
- **WHEN** a cell connects to 10.0.0.1
- **THEN** it receives `-ENETUNREACH`

### Requirement: Unix sockets are named in the cell's files
Binding an `AF_UNIX` stream socket to a path SHALL create a file of type
socket at that path in the cell's tree; connecting to that path SHALL reach
the socket listening there; once that socket is closed, connecting SHALL
fail with `ECONNREFUSED`.

#### Scenario: Unix echo
- **WHEN** a cell binds a Unix socket to `sock`, listens, connects to `sock`, accepts and exchanges bytes
- **THEN** the bytes arrive and `stat("sock")` reports a socket

### Requirement: Sockets across suspend and resume
A suspended cell SHALL hold no socket: on suspend its sockets are closed for
the network, and on resume a socket that was not bound is recreated, one
bound or listening is bound again at its address (and listens again), and
one that was connected (or connecting) is reset: its next call fails with
`ECONNRESET` and it polls as `POLLERR | POLLHUP`. A socket whose address is
taken at resume is reset. Data and connections not yet accepted are dropped.

#### Scenario: Listener survives, connection resets
- **WHEN** a cell holding a listening socket and an established connection is suspended and resumed
- **THEN** a new client can connect to the listener and be accepted, and reading the old connection fails with `ECONNRESET`

### Requirement: Sockets across fork
A forked cell SHALL share its parent's bound, listening and datagram
sockets, get a copy of its unbound sockets, and see its connected stream
sockets reset; the parent's sockets are unchanged.

#### Scenario: Forked servers
- **WHEN** a server cell waiting in `accept` is frozen as a template and two cells are spawned from it, and two clients connect
- **THEN** each spawned server accepts one connection and answers it

### Requirement: Waiting cells use no CPU
A cell waiting on descriptors SHALL not run until one of them may be ready
or its timeout ends; other cells run meanwhile.

#### Scenario: Server waits for its client
- **WHEN** a server cell blocks in `accept` and a client cell is spawned later
- **THEN** the server does not run until the client connects, then both finish
