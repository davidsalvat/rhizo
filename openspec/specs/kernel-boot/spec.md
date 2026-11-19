# kernel-boot Specification

## Purpose
Defines how the Rhizo kernel starts on its supported platform, what it reports
on the console while starting, and how it stops, either normally or after a
fatal error.

## Requirements

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

### Requirement: Boot banner on the serial console
The kernel SHALL print a banner on the boot console during boot that
identifies the project and the kernel version. The boot console is the fast
debug port when the VMM provides one and the first serial port otherwise.

#### Scenario: Banner is printed
- **WHEN** the kernel boots under QEMU with the serial port connected to the host
- **THEN** the host receives a line starting with `Rhizo` followed by the kernel version

#### Scenario: Banner on the debug port
- **WHEN** the kernel boots on a VMM that exposes the debug port and the host reads it
- **THEN** the host receives the banner line on the debug port

### Requirement: Clean shutdown after boot
In a normal (non-test) build, after completing boot the kernel SHALL report that
boot finished and stop the virtual machine. On VMMs with an exit device the VM
SHALL exit with the success status; on others the boot-complete line is the
success signal and the kernel stops the VM through the platform reset.

#### Scenario: Normal boot ends the VM successfully
- **WHEN** the kernel completes its boot sequence
- **THEN** the boot console shows a boot-complete message
- **AND** the virtual machine exits with the status reserved for success

#### Scenario: VMM without an exit device
- **WHEN** the kernel completes its boot sequence on Firecracker or Cloud Hypervisor
- **THEN** the boot console shows a boot-complete message and the VMM process ends

### Requirement: Panics are reported and fail the VM
When the kernel hits an unrecoverable error (a panic), it SHALL print the panic
message and its source location to the boot console and stop the virtual
machine with a failure status. It MUST NOT hang silently.

#### Scenario: Panic during boot
- **WHEN** kernel code panics
- **THEN** the boot console shows the panic message and the source file and line
- **AND** the virtual machine exits with the status reserved for failure

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
