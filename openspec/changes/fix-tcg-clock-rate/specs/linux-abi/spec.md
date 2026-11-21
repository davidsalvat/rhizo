## ADDED Requirements

### Requirement: The monotonic clock follows real time
The clock `clock_gettime` reads SHALL advance at the rate of real time
within 0.05% on every VMM, with or without KVM: on x86_64, the TSC
frequency the kernel finds (from KVM's clock, CPUID, the PIT or the local
APIC timer) SHALL agree within 0.05% with 50 ms of the local APIC timer,
which counts at 1 GHz on KVM and QEMU.

#### Scenario: Under emulation
- **WHEN** the kernel runs on QEMU without KVM (`microvm` or `q35`) and has found the TSC frequency
- **THEN** 50 ms of the local APIC timer converts to 50 ms of the clock within 0.05%

#### Scenario: A long wait measured in a cell
- **WHEN** a cell times, with `CLOCK_MONOTONIC`, a model that answers 65 s after its request
- **THEN** it reads at least 65 s, not 0.3 to 1.5% less
