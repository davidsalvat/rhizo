# networking Specification

## Purpose
How cells talk: pipes, event counters, `poll`/`select`/`epoll`, Unix and TCP/UDP sockets between cells, and a virtio-net card to reach other addresses, all carried by snapshots and forks.

## Requirements

### Requirement: Loopback network between cells
Cells of one kernel SHALL share a loopback network (127.0.0.0/8): a TCP
connection or a UDP datagram from one cell SHALL reach a socket of another
cell listening or bound at the destination address and port, in order and
without loss.

#### Scenario: Echo between two cells
- **WHEN** one cell listens on a loopback port and another connects, sends messages and reads the replies
- **THEN** each reply equals its message and both cells exit with status 0

### Requirement: Loopback addresses only
Binding to an address other than 127.0.0.0/8, `INADDR_ANY` or the network
card's address SHALL fail with `EADDRNOTAVAIL`; connecting or sending to an
address outside 127.0.0.0/8 SHALL go through the network card, and fail
with `ENETUNREACH` when the kernel has no card or no address. Port 0 SHALL
pick a free port in 32768–60999.

#### Scenario: Outside address
- **WHEN** a cell of a kernel without a network address connects to 10.0.0.1
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
Suspending a cell SHALL park each of its connected (or connecting) stream
sockets — TCP on the loopback network or through the network card, Unix
stream sockets and socket pairs — in the kernel: the connection stays open
with its peer, its sequence numbers and windows, its unread and
unacknowledged bytes and its options; the kernel keeps acknowledging and
retransmitting for it and keeps the bytes its peer sends, up to the
socket's receive bound.

#### Scenario: Listener and connection survive
- **WHEN** a cell holding a listening socket and an established loopback connection to itself is suspended and resumed
- **THEN** the connection is still open and carries bytes, and a new client can connect to the listener and be accepted

### Requirement: Receive bound of a parked connection
A parked connection SHALL keep the bytes its peer sends up to the socket's
receive bound: 256 KiB on loopback and Unix sockets, 64 KiB through the
card, where the advertised window then closes until the cell reads.

#### Scenario: Peer writes to a parked connection
- **WHEN** the peer of a parked loopback connection sends until it would block
- **THEN** it has sent exactly the receive bound and waits as for a slow reader, and after the resume the cell reads all of it and the peer can send again

### Requirement: Sockets after a resume
Resuming the snapshot in the same kernel SHALL give the cell the same
connection back: same peer, every byte in order, a peer's close or reset
during the suspend reported as it would have been without it.
Other sockets hold nothing while the cell is suspended: on resume a socket
that was not bound is recreated and a bound datagram socket is bound again
at its address (reset if the address is taken); datagrams queued on it are
dropped.

#### Scenario: Loopback connection across suspend of one end
- **WHEN** two cells exchange messages over a loopback TCP connection and one of them is suspended and resumed after every scheduling step until both exit
- **THEN** every message arrives once and in order and both cells exit with status 0

#### Scenario: Connection to the host across suspend
- **WHEN** a cell exchanging messages with the host's TCP echo server through the card is suspended repeatedly, with the card polled while it is suspended, and resumed each time
- **THEN** every echo arrives intact and the cell exits with status 0

### Requirement: Listeners across suspend
Listening sockets SHALL be parked the same way: the listener keeps its
address, connections made while the cell is suspended wait in its backlog
(beyond it they are refused, as for a busy server), and after the resume
the cell accepts them and those made before the suspend.

#### Scenario: Client of a suspended server
- **WHEN** a client connects to a loopback server between the server's suspend and its resume
- **THEN** the server accepts that connection after its resume and serves it

### Requirement: A parked socket comes back once
A parked socket SHALL be re-attached at most once, by the first resume of
its snapshot in the boot that parked it. Otherwise a connection comes back
reset: its next call fails with `ECONNRESET`, later reads return 0 and
later writes fail with `EPIPE`, and it polls as `POLLIN | POLLOUT | POLLHUP
| POLLRDHUP`, with `POLLERR` until the reset is reported; no call on it
waits. A listener is bound again at its address and listens again (reset
if the address is taken).

#### Scenario: Listener survives, connection resets
- **WHEN** a snapshot of a cell holding a listening socket and a parked connection is resumed, that cell exits, and a copy of the same snapshot is resumed
- **THEN** the second cell's connection reads `ECONNRESET`, then 0, writing to it fails with `EPIPE`, and a new client can connect to its listener and be accepted

#### Scenario: Another boot
- **WHEN** a snapshot record of a parked connection is restored by a kernel with another boot identity
- **THEN** the socket is reset

### Requirement: Discarded snapshots close their sockets
A snapshot discarded without being resumed SHALL close its parked sockets
(the peers of its connections see the end of the stream, connections
waiting on its listeners are reset).

#### Scenario: Discarded snapshot
- **WHEN** a suspended cell's snapshot is discarded
- **THEN** its parked sockets close and the kernel holds no socket of it

### Requirement: Sockets across fork
A forked cell, and a cell spawned from a template, SHALL share its parent's
listening and bound datagram sockets (both accept, or receive, from one
queue), get a copy of its sockets that were never bound, and see each
inherited connected (or connecting) stream socket reset. The parent's (or
template's) sockets are unchanged, and the peer of its connections sees
nothing of the fork.

#### Scenario: Forked servers
- **WHEN** a server cell waiting in `accept` is frozen as a template and two cells are spawned from it, and two clients connect
- **THEN** each spawned server accepts one connection and answers it

### Requirement: Inherited connections reset in a fork
The child's first call on an inherited connection (`read`, `write`,
`recv`, `send`, `connect` or `SO_ERROR`) SHALL fail with `ECONNRESET`;
later reads return 0 and later writes fail with `EPIPE` (the kernel sends
no signal: as with `SIGPIPE` ignored), `getpeername` fails with
`ENOTCONN`, and it polls as a reset socket.

#### Scenario: Connection of a fork
- **WHEN** a cell with an established loopback connection is forked
- **THEN** the child reads `ECONNRESET`, then 0, and writing fails with `EPIPE`, while the parent's connection still carries bytes both ways

### Requirement: Waiting cells use no CPU
A cell waiting on descriptors SHALL not run until one of them may be ready
or its timeout ends; other cells run meanwhile. A change to a socket SHALL
wake only the cells waiting on that socket.

#### Scenario: Server waits for its client
- **WHEN** a server cell blocks in `accept` and a client cell is spawned later
- **THEN** the server does not run until the client connects, then both finish

#### Scenario: A change wakes the waiters of its socket only
- **WHEN** a waiter blocked after looking at one connection and bytes are sent on another connection
- **THEN** the waiter is not woken; bytes sent on its own connection wake it once

### Requirement: Network card
The kernel SHALL drive a virtio network device attached through virtio-mmio
(version 2) on QEMU `virt`, QEMU `microvm` and Firecracker, found in the
device tree (AArch64) or in the `virtio_mmio.device=` command-line option
(x86_64), or, when there is none, through the modern virtio-pci transport
on QEMU `q35`, QEMU `virt` and Cloud Hypervisor, found on PCI bus 0 through
the configuration space the ACPI MCFG (x86_64) or the device tree's generic
ECAM host bridge (AArch64) gives.

#### Scenario: Card found
- **WHEN** the kernel boots on QEMU with a virtio-net device
- **THEN** it reports the card's MAC address and its network address

#### Scenario: Card found on PCI
- **WHEN** the kernel boots on QEMU `q35` with a `virtio-net-pci` device
- **THEN** it reports the card at `virtio-pci 00:DD.0` and the in-kernel test reaching the host's echo server passes

### Requirement: Network address
The kernel SHALL take its address from the command line: `ip=dhcp`
(DHCPv4) or `ip=ADDR/PREFIX` with an optional `,GATEWAY`.

#### Scenario: DHCP
- **WHEN** the kernel boots on QEMU's user network with `ip=dhcp`
- **THEN** it gets an address in 10.0.2.0/24 and cells can reach the host

### Requirement: Connections to the outside
A cell SHALL open TCP connections and exchange UDP datagrams with hosts
outside the kernel through the card with the same socket calls, blocking,
readiness and timeouts as on loopback; such connections SHALL be parked on
suspend, re-attached on resume and reset in forks like loopback
connections.

#### Scenario: Echo from the host
- **WHEN** a cell connects to a TCP echo server on the host, sends messages and reads the replies
- **THEN** each reply equals its message and the cell exits with status 0

### Requirement: Receive buffers
The network card driver SHALL keep receive buffers offered to the device
that add up to at least 65,562 bytes (a 64 KiB frame and its virtio-net
header) while frames wait for the kernel to take them, so that a device
that hands over a frame only when that much is offered (Firecracker) still
delivers bursts of frames.

#### Scenario: Device that waits for 64 KiB
- **WHEN** a device delivers frames only while the offered receive buffers add up to 65,562 bytes, and a burst arrives before the kernel polls the card
- **THEN** at least 16 frames of the burst are delivered, and the kernel receives them all

#### Scenario: Echo from the host on Firecracker
- **WHEN** the kernel boots on Firecracker with a tap device, `ip=172.16.0.2/24,172.16.0.1` and a TCP echo server on the host's tap address
- **THEN** a cell's messages to the echo server come back and the cell exits with status 0, with one CPU and with four

### Requirement: PCI memory BARs
The kernel SHALL keep the addresses the firmware or the VMM gave a virtio
network function's memory BARs and SHALL give those left without one a
naturally aligned address from the host bridge's 32-bit memory window
(AArch64); it SHALL then enable memory decoding and bus mastering. A
function whose virtio structures are missing or lie outside their BAR
SHALL be reported and skipped.

#### Scenario: QEMU virt without firmware
- **WHEN** the kernel boots on QEMU `virt` with a `virtio-net-pci` device
- **THEN** it assigns the BARs, brings the card up and reaches the host's echo server

### Requirement: TCP_NODELAY on a connection through the card
Setting `TCP_NODELAY` on a TCP connection through the network card SHALL
take effect at once, whether it was set before connecting or after.

#### Scenario: Set after connecting
- **WHEN** a cell sets `TCP_NODELAY` on an established connection to the outside and writes twice before the peer acknowledges the first write
- **THEN** the card sends both writes without waiting for the acknowledgment

### Requirement: Parked sockets answer only their own record
A listener or connection parked by a suspend SHALL be re-attached only by
a snapshot record carrying the key it was parked with. Keys SHALL be
derived from a secret the kernel holds and no snapshot contains, so that
no key can be guessed from the order of suspends or from other keys.

#### Scenario: Forged keys
- **WHEN** records with keys 0 to 63, and keys next to the keys of two parked connections, are imported
- **THEN** each comes back as a new socket in its record's own state, and both parked connections come back only for their own records

### Requirement: Ephemeral ports per boot
Ports given by `bind` to port 0 and by implicit binds SHALL be taken from
32768 to 60999, starting at a place chosen from the kernel's randomness of
the boot, then in order, skipping ports in use. A new boot SHALL therefore
not start its connections with the addresses and ports of the previous
boot's first connections, which a peer may still keep after closing them.

#### Scenario: Two boots
- **WHEN** 64 kernels with different boot identities each take one ephemeral port
- **THEN** every port is in the range and nearly all differ

#### Scenario: A host that keeps closed connections
- **WHEN** the laptop workload runs several times in a row on Firecracker or Cloud Hypervisor against one fake model server
- **THEN** every run's agent turns take about as long as on QEMU, not the 1 s of a connection retried after a refused first SYN

### Requirement: Wait queues per socket
Each socket SHALL keep a wait queue of the cells whose waiting calls looked
at it. These changes SHALL wake the waiters on its queue: bytes or a
datagram queued, room for a writer whose peer's queue was full, a
connection to accept, a shutdown, a close, a reset, and a change of a
connection through the network card. The queues SHALL NOT be part of a
cell's snapshot: a resumed or forked cell makes its waiting calls again.

#### Scenario: A writer waits for room
- **WHEN** a writer blocked on a full connection and the reader reads from it
- **THEN** the writer is woken; a later read from a queue that is no longer full wakes nobody

#### Scenario: A frame from the wire
- **WHEN** a waiter blocked on a connection through the network card and the answer arrives
- **THEN** the waiter is woken, and a waiter on a loopback socket is not
