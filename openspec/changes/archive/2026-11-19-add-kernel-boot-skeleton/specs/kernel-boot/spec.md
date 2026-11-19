# Spec Delta

## Purpose

Defines how the Rhizo kernel starts on its supported platform, what it reports
on the console while starting, and how it stops, either normally or after a
fatal error.

## ADDED Requirements

### Requirement: Direct boot on x86_64 virtual machines
The kernel SHALL be a single x86_64 ELF image that a hypervisor can boot
directly, without a separate bootloader or disk image. QEMU and other PVH-capable
hypervisors are the supported platforms.

#### Scenario: Boot under QEMU
- **WHEN** QEMU is started with the kernel image passed as the kernel to load
- **THEN** the kernel reaches its 64-bit entry point and starts executing kernel code

#### Scenario: Unsupported boot path
- **WHEN** the kernel is entered without the boot information the platform protocol guarantees
- **THEN** the kernel stops without executing further kernel code

### Requirement: Boot banner on the serial console
The kernel SHALL print a banner on the first serial port during boot that
identifies the project and the kernel version.

#### Scenario: Banner is printed
- **WHEN** the kernel boots under QEMU with the serial port connected to the host
- **THEN** the host receives a line starting with `Rhizo` followed by the kernel version

### Requirement: Clean shutdown after boot
In a normal (non-test) build, after completing boot the kernel SHALL report that
boot finished and stop the virtual machine with a success status.

#### Scenario: Normal boot ends the VM successfully
- **WHEN** the kernel completes its boot sequence
- **THEN** the serial console shows a boot-complete message
- **AND** the virtual machine exits with the status reserved for success

### Requirement: Panics are reported and fail the VM
When the kernel hits an unrecoverable error (a panic), it SHALL print the panic
message and its source location to the serial console and stop the virtual
machine with a failure status. It MUST NOT hang silently.

#### Scenario: Panic during boot
- **WHEN** kernel code panics
- **THEN** the serial console shows the panic message and the source file and line
- **AND** the virtual machine exits with the status reserved for failure

### Requirement: Boot time is reported
The kernel SHALL report on the serial console how long boot took, measured from
kernel entry to boot completion, so boot-time regressions are visible.

#### Scenario: Boot time line
- **WHEN** boot completes
- **THEN** the serial console shows the boot duration with its unit
