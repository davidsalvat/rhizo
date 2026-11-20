# Proposal

## Why

`add-virtio-net` drives virtio network cards on virtio-mmio only, so Cloud
Hypervisor and QEMU `q35`, which attach virtio devices only through PCI,
run without a card: cells there cannot reach the outside, the in-kernel
network test does not run, and `net.tcp_rtt_us` is skipped. Cloud
Hypervisor is one of the three VMMs the project targets.

## What Changes

- `rhizo-virtio` gets the modern virtio-pci transport (virtio 1.2,
  §4.1): recognizing a virtio network function, sizing its memory BARs,
  giving an address to BARs the firmware left without one, enabling
  decoding and bus mastering, and parsing the vendor capabilities that
  locate the common, notification and device configuration structures.
  The virtqueue and network logic is shared with virtio-mmio.
- `rhizo-hal`'s `DeviceRegisters` gains 8- and 16-bit accesses: virtio-pci
  registers must be accessed with their own width.
- `rhizo-core` reads the PCI configuration space's address: the ACPI MCFG
  (x86_64) and the device tree's `pci-host-ecam-generic` node with its
  32-bit memory window (AArch64).
- The kernel scans PCI bus 0 through ECAM when no virtio-mmio card is
  found, maps the configuration pages and the structures behind the
  `arch` interface (`platform::{pci_ecam, pci_window, map_device}`), and
  polls the card as before.
- `xtask` gives QEMU `q35` a `virtio-net-pci` card on the user network,
  Cloud Hypervisor one on a tap device of its own (when `sudo` allows it),
  and QEMU `virt` a `virtio-net-pci` card in its 4-CPU in-kernel test run
  (the 1-CPU run keeps virtio-mmio). `cargo xtask ci` on x86_64 also runs
  the in-kernel tests on `q35`.

### Out of scope

Interrupts from the card (still polled; MSI-X stays disabled), PCI
bridges and buses other than 0, multi-function devices, legacy (port I/O)
configuration access (every supported VMM has ECAM), assigning BARs on
x86_64 (the firmware or the VMM does), other virtio-pci devices.

## Capabilities

### Modified Capabilities

- `networking`: the network card is also found on PCI.
- `benchmarks`: `net.tcp_rtt_us` on `q35` and Cloud Hypervisor.

## Impact

`crates/rhizo-hal` (narrow register accesses), `crates/rhizo-virtio` (PCI
transport), `crates/rhizo-core` (MCFG, PCI window), `kernel` (PCI scan in
`wire.rs`, device mapping on both architectures), `xtask` (VMM network
configuration, `q35` in-kernel tests in `ci`), `bench/README.md`,
AGENTS.md. No new crate or dependency edge. Release kernels without cells
are unchanged.
