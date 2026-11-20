# Spec Delta

## MODIFIED Requirements

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

## ADDED Requirements

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
