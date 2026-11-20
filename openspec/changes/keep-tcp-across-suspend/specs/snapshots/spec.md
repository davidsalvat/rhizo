# Spec Delta

## MODIFIED Requirements

### Requirement: Pipes and sockets in snapshots
A snapshot (format version 6) SHALL contain the cell's pipes with their
data, its event counters, its `epoll` instances and one record per socket
(family, type, state, address, options; for a parked listener or
connection, the boot identity of the kernel that parked it and its key
there); resuming SHALL
restore the first three as they were and the sockets as the `networking`
capability says. A record whose fields are out of range SHALL make the
snapshot invalid.

#### Scenario: Pipe across suspend
- **WHEN** a cell writes into a pipe, is suspended and resumed, and reads the pipe
- **THEN** it reads the bytes written before the suspend

#### Scenario: Blocked cell across suspend
- **WHEN** a cell blocked in a system call is suspended and resumed
- **THEN** it waits in the same call again, and completes it once a descriptor is ready

#### Scenario: Parked record round trip
- **WHEN** socket records, a parked one among them, are encoded and decoded
- **THEN** the records are equal, and a truncated or out-of-range record is refused
