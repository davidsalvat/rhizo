# device-tree Specification

## Purpose
Defines how the kernel reads a flattened device tree passed at boot: memory,
the command line, the PSCI conduit and the boot UART.

## Requirements

### Requirement: Memory comes from the device tree
On platforms that boot with a device tree, the kernel SHALL take its RAM
regions from the `reg` property of the `memory` nodes, honoring the root
`#address-cells` and `#size-cells`, including several banks in one property.

#### Scenario: QEMU virt
- **WHEN** the kernel boots on QEMU `virt` with 128 MiB of RAM
- **THEN** it uses the region starting at `0x4000_0000` of 128 MiB

#### Scenario: Two banks
- **WHEN** a memory node lists two `(base, size)` pairs
- **THEN** both regions become usable RAM

### Requirement: Command line, PSCI and UART from the device tree
The kernel SHALL read its command line from `/chosen/bootargs`, the PSCI
conduit from `/psci/method`, and its boot UART from the first node compatible
with `arm,pl011` or `ns16550a`, using the parent node's cell sizes for `reg`.

#### Scenario: Options on the command line
- **WHEN** `bootargs` is `console=serial exit=psci`
- **THEN** the kernel uses the serial console and ends the VM through PSCI

#### Scenario: UART under a bus with 2-cell addresses
- **WHEN** a `ns16550a` node sits under a node with `#address-cells = <2>`
- **THEN** its register base is read as a 64-bit address

### Requirement: Invalid device trees are rejected
The reader SHALL reject blobs without the device-tree magic, with a format
version older than 16, or whose blocks or tokens run past the declared size,
and the kernel SHALL stop with a panic that names the problem.

#### Scenario: Truncated blob
- **WHEN** the device tree is cut short
- **THEN** reading it fails with a truncation error and nothing past its end is read

### Requirement: Device tree memory is reserved
The physical memory holding the device tree SHALL never be handed out by the
frame allocator.

#### Scenario: Allocating everything
- **WHEN** every free frame is allocated
- **THEN** none overlaps the device tree

### Requirement: CPUs from the device tree
The kernel SHALL read its CPUs from the children of `/cpus` whose
`device_type` is `cpu`: the `reg` value (with the `#address-cells` of
`/cpus`) as the CPU's affinity, whether `enable-method` lists `psci`, and
SHALL skip CPUs whose `status` is neither `okay` nor `ok`.

#### Scenario: QEMU virt with four CPUs
- **WHEN** the device tree of `-M virt -smp 4` is read
- **THEN** it lists four CPUs, affinities 0 to 3, each started through PSCI

#### Scenario: Two-cell identifiers and disabled CPUs
- **WHEN** `/cpus` has `#address-cells = <2>`, a disabled CPU and a node that is not a CPU
- **THEN** identifiers are read as 64-bit values, and only the available CPUs are listed
