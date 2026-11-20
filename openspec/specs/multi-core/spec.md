# multi-core Specification

## Purpose
How the kernel brings every CPU the firmware lists online and runs cells on all of them in parallel, each cell on one CPU at a time, with the same behavior as one CPU.

## Requirements

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

### Requirement: Cells isolated on every CPU
Cells running at the same time on different CPUs SHALL stay isolated from
each other exactly as on one CPU: each sees only its own memory, files,
registers and FP/SIMD state, and every frame and tag returns when they
end. A cell's FP/SIMD state and address space SHALL leave a CPU with the
cell, so the next CPU to run it continues it unchanged.

#### Scenario: Many programs at once
- **WHEN** four copies each of `counter`, `filer`, `libc_threads`, `chatter` and `threads` run at once on 4 vCPUs
- **THEN** each prints exactly what it prints alone and exits with 0, and all memory comes back

### Requirement: TLB coherence across CPUs
No CPU SHALL use a stale translation of a cell: one that an address-space
change of the cell (unmap, protection change, copy-on-write, fork) removed
or narrowed, or one that a previous holder of the cell's tag left. Where
invalidation is per CPU (x86_64), a CPU SHALL drop the cell's tag before it
next runs the cell if the cell's translations shrank, or the cell got its
tag, since that CPU last dropped it.

#### Scenario: Copy-on-write on several CPUs
- **WHEN** forks of a cell and templates run, write and exit on 4 vCPUs
- **THEN** each sees only its own writes, as on one CPU

### Requirement: CPUs let go of the cells they give back
A CPU SHALL no longer use a cell's tables or hold its FP/SIMD state once it
has given the cell back, so another CPU can run the cell, or free it, at
once.

#### Scenario: A cell that moves
- **WHEN** two cells computing with different FP/SIMD values yield to each other on 4 vCPUs, each moving between CPUs
- **THEN** each gets its exact result

### Requirement: Cells stopped before host operations
Suspend, resume, fork, freeze, kill and spawn SHALL act on cells none of
whose threads runs on any CPU: they happen between runs, after every CPU
gave back its cell.

#### Scenario: Suspend between parallel runs
- **WHEN** `counter` runs next to spinning cells on 4 vCPUs, and is suspended and resumed from its snapshot's bytes after every few slices until it exits
- **THEN** its whole output is the uninterrupted one

### Requirement: Shared copy-on-write memory changes one CPU at a time
Changes to page tables and frames that cells share copy-on-write (resolving
a write, unmapping, changing protections, freeing) SHALL be made by one CPU
at a time, so reference counts stay exact; cells that share no memory SHALL
not wait for each other.

#### Scenario: Children of one parent at once
- **WHEN** children forked from one cell write to and free shared pages on several CPUs at the same time, then exit
- **THEN** every shared frame and table is returned

### Requirement: Kernel allocations survive fragmentation
A large kernel heap allocation SHALL succeed whenever enough frames are
free, contiguous or not.

#### Scenario: No two free frames together
- **WHEN** every other free frame is taken and the kernel allocates 1 MiB
- **THEN** the allocation succeeds, holds what is written, and its frames come back when it is freed
