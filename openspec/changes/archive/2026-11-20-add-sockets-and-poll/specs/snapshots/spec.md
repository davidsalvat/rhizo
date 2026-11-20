# Spec Delta

## ADDED Requirements

### Requirement: Pipes and sockets in snapshots
A snapshot (format version 4) SHALL contain the cell's pipes with their
data, its event counters, its `epoll` instances and one record per socket
(family, type, state, address, options); resuming SHALL restore the first
three as they were and the sockets as the `networking` capability says.

#### Scenario: Pipe across suspend
- **WHEN** a cell writes into a pipe, is suspended and resumed, and reads the pipe
- **THEN** it reads the bytes written before the suspend

#### Scenario: Blocked cell across suspend
- **WHEN** a cell blocked in a system call is suspended and resumed
- **THEN** it waits in the same call again, and completes it once a descriptor is ready
