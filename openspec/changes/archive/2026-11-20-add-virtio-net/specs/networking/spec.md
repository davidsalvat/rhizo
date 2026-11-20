# Spec Delta

## MODIFIED Requirements

### Requirement: Loopback addresses only
Binding to an address other than 127.0.0.0/8, `INADDR_ANY` or the network
card's address SHALL fail with `EADDRNOTAVAIL`; connecting or sending to an
address outside 127.0.0.0/8 SHALL go through the network card, and fail
with `ENETUNREACH` when the kernel has no card or no address. Port 0 SHALL
pick a free port in 32768–60999.

#### Scenario: Outside address
- **WHEN** a cell of a kernel without a network address connects to 10.0.0.1
- **THEN** it receives `-ENETUNREACH`

## ADDED Requirements

### Requirement: Network card
The kernel SHALL drive a virtio network device attached through virtio-mmio
(version 2) on QEMU `virt`, QEMU `microvm` and Firecracker, found in the
device tree (AArch64) or in the `virtio_mmio.device=` command-line option
(x86_64). On VMMs that attach virtio devices only through PCI (Cloud
Hypervisor, QEMU `q35`) the kernel SHALL run without a card.

#### Scenario: Card found
- **WHEN** the kernel boots on QEMU with a virtio-net device
- **THEN** it reports the card's MAC address and its network address

### Requirement: Network address
The kernel SHALL take its address from the command line: `ip=dhcp`
(DHCPv4) or `ip=ADDR/PREFIX` with an optional `,GATEWAY`.

#### Scenario: DHCP
- **WHEN** the kernel boots on QEMU's user network with `ip=dhcp`
- **THEN** it gets an address in 10.0.2.0/24 and cells can reach the host

### Requirement: Connections to the outside
A cell SHALL open TCP connections and exchange UDP datagrams with hosts
outside the kernel through the card with the same socket calls, blocking,
readiness and timeouts as on loopback; such connections SHALL be reset on
suspend, resume and fork like loopback connections.

#### Scenario: Echo from the host
- **WHEN** a cell connects to a TCP echo server on the host, sends messages and reads the replies
- **THEN** each reply equals its message and the cell exits with status 0
