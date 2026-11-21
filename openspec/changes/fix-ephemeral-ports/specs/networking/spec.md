## ADDED Requirements

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
