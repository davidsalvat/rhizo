# Spec Delta

## MODIFIED Requirements

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
kernel entry to boot completion, and SHALL print a machine-readable ready line
the host can use to timestamp the end of boot.

#### Scenario: Boot time line
- **WHEN** boot completes
- **THEN** the boot console shows the boot duration with its unit

#### Scenario: Ready marker
- **WHEN** boot completes
- **THEN** the boot console shows a line `rhizo-ready cycles=<n>` before any shutdown message
