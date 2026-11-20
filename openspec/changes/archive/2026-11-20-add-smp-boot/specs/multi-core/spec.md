# Spec Delta

## ADDED Requirements

### Requirement: CPUs from the firmware
A kernel build that runs cells SHALL use the boot CPU and the other CPUs the
firmware describes, in firmware order: on AArch64 the `/cpus` nodes of the
device tree whose `enable-method` is `psci`, on x86_64 the enabled or
online-capable processors of the ACPI MADT (local APIC and x2APIC entries),
or, without ACPI, the enabled processors of the MP configuration table.
It SHALL use at most 16 CPUs and report `cpus: <n> online` on the boot
console once they are up.

#### Scenario: Four vCPUs
- **WHEN** the in-kernel test build boots on a VM with 4 vCPUs on QEMU, Firecracker or Cloud Hypervisor
- **THEN** the console shows `cpus: 4 online` and every in-kernel test passes

#### Scenario: One vCPU
- **WHEN** a kernel boots on a VM with one vCPU
- **THEN** it starts no other CPU and the console shows `cpus: 1 online`

### Requirement: CPU limit on the command line
The `smp=N` boot option (`N` from 1 to 65535) SHALL cap the number of CPUs
the kernel uses; with `smp=1` the kernel SHALL start no other CPU and
behave as a single-CPU kernel. A malformed value SHALL be ignored.

#### Scenario: smp=2 on four vCPUs
- **WHEN** the kernel boots with `smp=2` on a VM with 4 vCPUs
- **THEN** two CPUs are online

#### Scenario: Malformed limit
- **WHEN** the command line holds `smp=0` or `smp=x`
- **THEN** the kernel uses every CPU the firmware describes

### Requirement: Per-CPU stacks and data
Each CPU SHALL have its own kernel stack with an unmapped guard page below
it, its own exception stacks, its own number (0 for the boot CPU) readable
in constant time, and its own interrupt controller state (GICv3
redistributor or local APIC). A cell SHALL NOT be able to change where the
kernel finds its per-CPU data.

#### Scenario: Numbers and stacks
- **WHEN** a function runs on each CPU in turn
- **THEN** each reports its own number and runs on its own stack, above an unmapped guard page

### Requirement: Idle CPUs use no time
A CPU with nothing to do SHALL wait for an interrupt without a periodic
tick, and SHALL resume when another CPU hands it work and sends it a
wake-up interrupt, without missing a wake-up sent just before it waits.

#### Scenario: Work for an idle CPU
- **WHEN** the boot CPU hands a function to an idle CPU
- **THEN** the idle CPU runs it and the boot CPU gets its result

### Requirement: Kernel globals across CPUs
Kernel globals (frame allocator, heap, and every other `sync::Global`)
SHALL be safe to use from all CPUs at once: a CPU that finds a global held
by another CPU SHALL wait for it, and a CPU that asks again for a global it
holds SHALL panic (reentrance is a bug).

#### Scenario: All CPUs at once
- **WHEN** every CPU increments a shared global and allocates and frees kernel heap many times at the same time
- **THEN** no increment is lost and every allocation is freed

#### Scenario: Reentrance
- **WHEN** a CPU asks for a global it already holds
- **THEN** the kernel panics instead of deadlocking
