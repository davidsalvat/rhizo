# Spec Delta

## RENAMED Requirements

- FROM: `### Requirement: Direct boot on x86_64 virtual machines`
- TO: `### Requirement: Direct boot on virtual machines`

## MODIFIED Requirements

### Requirement: Direct boot on virtual machines
The kernel SHALL boot directly from a hypervisor, without a separate
bootloader or disk image, through the architecture's standard protocol:
a flat image with the Linux arm64 Image header and a device tree on AArch64
(entered at EL1, or at EL2 and then dropped to EL1), and a PVH ELF image on
x86_64.

#### Scenario: Boot under QEMU
- **WHEN** QEMU is started with the kernel image passed as the kernel to load
- **THEN** the kernel reaches its 64-bit entry point and starts executing kernel code

#### Scenario: Entered at EL2
- **WHEN** an AArch64 VMM enters the kernel at EL2
- **THEN** the kernel continues at EL1 and boots normally

#### Scenario: Unsupported boot path
- **WHEN** the kernel is entered without the boot information the platform protocol guarantees
- **THEN** the kernel stops without executing further kernel code

### Requirement: Boot time is reported
The kernel SHALL report on the boot console how long boot took, measured from
kernel entry to boot completion, in counter cycles and, when the architecture
reports its counter frequency, in microseconds; and SHALL print a
machine-readable ready line the host can use to timestamp the end of boot.

#### Scenario: Boot time line
- **WHEN** boot completes
- **THEN** the boot console shows the boot duration with its unit

#### Scenario: Microseconds on AArch64
- **WHEN** boot completes on AArch64
- **THEN** the boot time line also shows the duration in microseconds

#### Scenario: Ready marker
- **WHEN** boot completes
- **THEN** the boot console shows a line `rhizo-ready cycles=<n>` before any shutdown message
