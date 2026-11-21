## MODIFIED Requirements

### Requirement: Ephemeral ports per boot
Ports given by `bind` to port 0 and by implicit binds SHALL be taken from
32768 to 60999, in order from a start chosen from the kernel's randomness
of the boot (for each cell, as "Ephemeral ports per cell" says), skipping
ports in use. A new boot SHALL therefore not start its connections with
the addresses and ports of the previous boot's first connections, which a
peer may still keep after closing them.

#### Scenario: Two boots
- **WHEN** 64 kernels with different boot identities each take one ephemeral port
- **THEN** every port is in the range and nearly all differ

#### Scenario: A host that keeps closed connections
- **WHEN** the laptop workload runs several times in a row on Firecracker or Cloud Hypervisor against one fake model server
- **THEN** every run's agent turns take about as long as on QEMU, not the 1 s of a connection retried after a refused first SYN

## ADDED Requirements

### Requirement: Ephemeral ports per cell
Each cell SHALL take its ephemeral ports in order from a start of its own,
chosen from the kernel's secret for the cell's identifier, so that the
ports a cell gets do not depend on how many ports other cells took.
Sockets the kernel opens for itself SHALL keep an order of their own. A
resumed or forked cell, which has a new identifier, SHALL start elsewhere
than the cell it came from.

#### Scenario: Other cells' binds
- **WHEN** a cell binds a socket to port 0, another cell binds 100 sockets to port 0 and closes them, and the first cell binds another
- **THEN** the first cell's second port follows its first

#### Scenario: Starts of different cells
- **WHEN** 64 cells each take their first ephemeral port
- **THEN** every port is in the range and nearly all differ
