# Spec Delta

## MODIFIED Requirements

### Requirement: Sockets across suspend and resume
Suspending a cell SHALL park each of its connected (or connecting) stream
sockets — TCP on the loopback network or through the network card, Unix
stream sockets and socket pairs — in the kernel: the connection stays open
with its peer, its sequence numbers and windows, its unread and
unacknowledged bytes and its options; the kernel keeps acknowledging and
retransmitting for it and keeps the bytes its peer sends, up to the
socket's receive bound (256 KiB on loopback and Unix sockets, 64 KiB
through the card, where the advertised window then closes until the cell
reads). Resuming the snapshot in the same kernel SHALL give the cell the
same connection back: same peer, every byte in order, a peer's close or
reset during the suspend reported as it would have been without it.

Listening sockets SHALL be parked the same way: the listener keeps its
address, connections made while the cell is suspended wait in its backlog
(beyond it they are refused, as for a busy server), and after the resume
the cell accepts them and those made before the suspend.

A parked socket SHALL be re-attached at most once, by the first resume of
its snapshot in the boot that parked it. Otherwise a connection comes back
reset: its next call fails with `ECONNRESET`, later reads return 0 and
later writes fail with `EPIPE`, and it polls as `POLLIN | POLLOUT | POLLHUP
| POLLRDHUP`, with `POLLERR` until the reset is reported; no call on it
waits. A listener is bound again at its address and listens again (reset
if the address is taken). A snapshot discarded without being resumed SHALL
close its parked sockets (the peers of its connections see the end of the
stream, connections waiting on its listeners are reset).

Other sockets hold nothing while the cell is suspended: on resume a socket
that was not bound is recreated and a bound datagram socket is bound again
at its address (reset if the address is taken); datagrams queued on it are
dropped.

#### Scenario: Listener and connection survive
- **WHEN** a cell holding a listening socket and an established loopback connection to itself is suspended and resumed
- **THEN** the connection is still open and carries bytes, and a new client can connect to the listener and be accepted

#### Scenario: Loopback connection across suspend of one end
- **WHEN** two cells exchange messages over a loopback TCP connection and one of them is suspended and resumed after every scheduling step until both exit
- **THEN** every message arrives once and in order and both cells exit with status 0

#### Scenario: Connection to the host across suspend
- **WHEN** a cell exchanging messages with the host's TCP echo server through the card is suspended repeatedly, with the card polled while it is suspended, and resumed each time
- **THEN** every echo arrives intact and the cell exits with status 0

#### Scenario: Listener survives, connection resets
- **WHEN** a snapshot of a cell holding a listening socket and a parked connection is resumed, that cell exits, and a copy of the same snapshot is resumed
- **THEN** the second cell's connection reads `ECONNRESET`, then 0, writing to it fails with `EPIPE`, and a new client can connect to its listener and be accepted

#### Scenario: Another boot
- **WHEN** a snapshot record of a parked connection is restored by a kernel with another boot identity
- **THEN** the socket is reset

#### Scenario: Discarded snapshot
- **WHEN** a suspended cell's snapshot is discarded
- **THEN** its parked sockets close and the kernel holds no socket of it

#### Scenario: Client of a suspended server
- **WHEN** a client connects to a loopback server between the server's suspend and its resume
- **THEN** the server accepts that connection after its resume and serves it

### Requirement: Sockets across fork
A forked cell, and a cell spawned from a template, SHALL share its parent's
listening and bound datagram sockets (both accept, or receive, from one
queue), get a copy of its sockets that were never bound, and see each
inherited connected (or connecting) stream socket reset: the child's first
call on it (`read`, `write`, `recv`, `send`, `connect` or `SO_ERROR`) fails
with `ECONNRESET`, later reads return 0 and later writes fail with `EPIPE`
(the kernel sends no signal: as with `SIGPIPE` ignored), `getpeername`
fails with `ENOTCONN`, and it polls as a reset socket. The parent's (or
template's) sockets are unchanged, and the peer of its connections sees
nothing of the fork.

#### Scenario: Forked servers
- **WHEN** a server cell waiting in `accept` is frozen as a template and two cells are spawned from it, and two clients connect
- **THEN** each spawned server accepts one connection and answers it

#### Scenario: Connection of a fork
- **WHEN** a cell with an established loopback connection is forked
- **THEN** the child reads `ECONNRESET`, then 0, and writing fails with `EPIPE`, while the parent's connection still carries bytes both ways

### Requirement: Connections to the outside
A cell SHALL open TCP connections and exchange UDP datagrams with hosts
outside the kernel through the card with the same socket calls, blocking,
readiness and timeouts as on loopback; such connections SHALL be parked on
suspend, re-attached on resume and reset in forks like loopback
connections.

#### Scenario: Echo from the host
- **WHEN** a cell connects to a TCP echo server on the host, sends messages and reads the replies
- **THEN** each reply equals its message and the cell exits with status 0
