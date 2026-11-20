# Proposal

## Why

Rhizo runs on one CPU. Agent workloads, and the density the project is
about, need every core of the machine: many cells side by side, and the
threads of one cell (H14) on several cores. Before cells can run in
parallel the kernel has to bring the other CPUs up, give each its own
stacks, per-CPU data and interrupt controller, and make its globals safe
when two CPUs reach them at once. This change (the first of milestone H17,
multi-core) does that and nothing more: the other CPUs come online and
idle; cells still run on the boot CPU. `add-smp-scheduling` then runs cells
on all of them.

## What Changes

- **CPU discovery**: AArch64 reads `/cpus` from the device tree (CPUs with
  `enable-method = "psci"`); x86_64 reads the processors of the ACPI MADT
  (RSDP from the PVH start info, else the EBDA and BIOS area). The boot
  CPU is CPU 0; others follow in firmware order. Without ACPI (QEMU
  `microvm` with `acpi=off`), x86_64 reads the MP configuration table.
- **`smp=N`** on the command line caps the CPUs used (default: every CPU
  the firmware describes, at most 16). `smp=1`, and any VM with one vCPU,
  starts nothing and behaves as before.
- **Bring-up**: AArch64 with PSCI `CPU_ON` into an entry that takes the
  boot identity map to the higher half; x86_64 with INIT and STARTUP IPIs
  into a real-mode trampoline at 0x8000 that goes through protected mode
  to long mode on a temporary root (kernel half plus the trampoline page).
  Only builds that run cells (`ktest`, `bench-cell`) start CPUs: the plain
  boot path (and its benchmarks) does not change.
- **Per-CPU state**: kernel, exception and double-fault stacks mapped at
  `0xffff_ffff_c000_0000 + 128 KiB * n` with unmapped guard pages; per-CPU
  data through `TPIDR_EL1` (AArch64) or the kernel GS base (x86_64, with
  `SWAPGS` around cell code so a cell can never point the kernel's GS at
  its memory); a GDT and TSS per CPU on x86_64; each CPU's own GICv3
  redistributor (found by affinity) or local APIC.
- **Idle and wake-up**: an idle CPU waits in `wfi`/`hlt` (no tick) and is
  woken by an IPI (SGI 0, vector 33) when it is handed work.
- **Locks**: `sync::Global` becomes a spinlock that knows its holder
  (`rhizo_core::sync::SpinLock`): another CPU waits, the same CPU asking
  again still panics (reentrance is a bug, as before).
- **Heap pages from the top of memory**, so the pages the heap keeps do
  not split the free runs that copy-on-write large pages and snapshots
  need (with several CPUs, a template child lost a 2 MiB copy-on-write);
  and a copy-on-write large page that finds no free 2 MiB block is copied
  into small pages instead of killing the cell.
- **Tests**: host tests for the MADT reader, the CPU list of the device
  tree, CPU selection (property test) and the lock (threads as CPUs); in
  kernel, every CPU's number and stack, and four CPUs hammering a global
  and the kernel heap at once. `cargo xtask test` gains a `ktest-smp`
  step: the whole in-kernel suite with 4 vCPUs (`-smp 4`, Firecracker
  `vcpu_count`, Cloud Hypervisor `--cpus boot=4`), which must report
  `cpus: 4 online`. CI also runs it on Firecracker and Cloud Hypervisor.

### Out of scope

Running cells on the other CPUs, per-CPU run queues, load balancing, TLB
shootdowns and stopping a cell on several CPUs (all `add-smp-scheduling`);
CPU hot-plug and taking CPUs offline; GICv2; x2APIC identifiers above 255
in xAPIC mode; more than 16 CPUs.

## Capabilities

### New Capabilities

- `multi-core`: which CPUs the kernel uses, how they start, their per-CPU
  state, idling and wake-up, and the locks of kernel globals.

### Modified Capabilities

- `device-tree`: the CPUs come from `/cpus` too.
- `kernel-testing`: the in-kernel suite also runs on 4 vCPUs (`ktest-smp`).
- `fork`: copy-on-write of a large page without a free large frame.

## Impact

`crates/rhizo-core` (`acpi`, `smp`, `sync`, `fdt::cpus`, `smp=` option,
`boot::report_cpus`), `crates/rhizo-mm` (`FrameAllocator::alloc_high`),
`kernel/src/heap.rs`, `kernel/src/sync.rs`, `kernel/src/smp.rs` (new),
`kernel/src/arch/aarch64/{boot,cpu,irq,platform,smp}.rs`,
`kernel/src/arch/x86_64/{boot,cpu,irq,user,smp}.rs`, `kernel/src/cell.rs`
(interrupts and tags set up once at boot), `kernel/src/tests.rs`,
`xtask` (`RunOptions::cpus`, `ktest-smp`), `.github/workflows/ci.yml`,
`AGENTS.md`.
