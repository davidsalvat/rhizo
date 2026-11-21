# Spec Delta

## MODIFIED Requirements

### Requirement: Loopback network between cells
Each cell SHALL have a loopback network (127.0.0.0/8) of its own, unless
its creator gives it another cell's to share. A TCP connection or a UDP
datagram SHALL reach a socket listening or bound at the destination
address and port in the sender's loopback network, in order and without
loss, and SHALL never reach another network.

#### Scenario: Echo between two cells
- **WHEN** one cell listens on a loopback port and another cell sharing its loopback network connects, sends messages and reads the replies
- **THEN** each reply equals its message and both cells exit with status 0

#### Scenario: Separate networks
- **WHEN** one cell listens on 127.0.0.1:7300 and a cell with its own loopback network connects to that address and then binds that port
- **THEN** the connection is refused with `ECONNREFUSED` and the bind succeeds

### Requirement: Loopback addresses only
Binding to an address other than 127.0.0.0/8, `INADDR_ANY` or the network
card's address SHALL fail with `EADDRNOTAVAIL`. Connecting or sending to
an address outside 127.0.0.0/8 SHALL go through the network card if the
cell's egress policy allows it, fail with `EACCES` if not, and with
`ENETUNREACH` when the kernel has no card or no address. Port 0 SHALL
pick a free port of the cell's loopback network in 32768–60999.

#### Scenario: Outside address
- **WHEN** a cell of a kernel without a network address connects to 10.0.0.1
- **THEN** it receives `-ENETUNREACH`

### Requirement: Ephemeral ports per boot
Each loopback network SHALL give ports for `bind` to port 0 and implicit
binds from 32768 to 60999, starting at a place chosen from the kernel's
randomness of the boot, then in order, skipping its own ports in use. A
socket through the card SHALL keep its network's port for its cell, while
the card port is chosen by the kernel at random among free ones and is
shown to no cell.

#### Scenario: Two boots
- **WHEN** 64 kernels with different boot identities each take one ephemeral port
- **THEN** every port is in the range and nearly all differ

#### Scenario: A host that keeps closed connections
- **WHEN** the laptop workload runs several times in a row on Firecracker or Cloud Hypervisor against one fake model server
- **THEN** every run's agent turns take about as long as on QEMU, not the 1 s of a connection retried after a refused first SYN

#### Scenario: Ports of other domains
- **WHEN** a cell takes 1000 ports and connections to the outside while a cell of another domain takes ephemeral ports and reads them with `getsockname`
- **THEN** the second cell reads the same ports as when it runs alone

### Requirement: Sockets across fork
A forked cell, and a cell spawned from a template, in its source's
loopback network SHALL share its source's listening and bound datagram
sockets (both accept, or receive, from one queue); in another network
those SHALL be bound again there. It SHALL get a copy of sockets never
bound and see inherited connected (or connecting) stream sockets reset;
the source and its peers see nothing of the fork.

#### Scenario: Forked servers
- **WHEN** a server cell waiting in `accept` is frozen as a template and two cells are spawned from it into its loopback network, and two clients connect
- **THEN** each spawned server accepts one connection and answers it

#### Scenario: Copy in its own network
- **WHEN** a server template is copied into a new loopback network and a client in that network connects
- **THEN** the copy accepts it, and a client of the template's network is never accepted by the copy

## ADDED Requirements

### Requirement: Egress policy per cell
Each cell SHALL have an egress policy: `none`, `model` (the endpoints the
host names as the model), a list of rules (protocol, IPv4 prefix, port
range) or `full`, all within a floor of addresses the host denies to
every cell. `connect` and datagrams to a destination outside it SHALL
fail with `EACCES`; loopback SHALL NOT depend on it.

#### Scenario: Each level
- **WHEN** cells with egress `none`, `model`, a rule for the host's echo server, and `full` each connect to the model endpoint, to the echo server and to another host address
- **THEN** each succeeds exactly where its policy allows and receives `EACCES` elsewhere

### Requirement: Egress only narrows
A child's egress policy SHALL be within its parent's, a moved cell's
SHALL become its intersection with the new parent's, and only the host
SHALL widen a policy (that of a root, or within the parent's). Narrowing
a live cell's policy SHALL reset its connections the new one denies.

#### Scenario: Child asks for more
- **WHEN** a cell with egress `model` is given a child with egress `full`
- **THEN** the creation is refused and no cell is made

### Requirement: Inbound datagrams follow the policy
A datagram from the network SHALL reach a cell's socket only if the
cell's egress policy allows its source address and port; others SHALL be
dropped.

#### Scenario: Stranger's datagram
- **WHEN** a host outside the policy of a cell with egress `model` sends UDP datagrams to that cell's bound port
- **THEN** the cell receives none of them, and receives those the model endpoint sends

### Requirement: Sockets stay with their cell
A socket SHALL carry the egress policy of the cell that made it and SHALL
reach no other cell, except the copies of its own cell made by a fork,
which have the same policy.

#### Scenario: Borrowed connection
- **WHEN** a cell with egress `none` receives, over a channel, a connected socket of a cell with egress `full`
- **THEN** the transfer is refused with `EPERM`, and nothing the first cell does puts a byte on the card
