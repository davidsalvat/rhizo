# Proposal

## Why

H2 runs one cell at a time to completion. Agent hosts run many cells at once,
most of them idle, and run untrusted code: a cell that spins forever must not
stop the others, and no cell may see another's memory. H3 makes several cells
live at once under a preemptive scheduler, proves their isolation with tests,
and makes switching between cells cheap by tagging TLB entries per cell
(ASIDs on AArch64, PCIDs on x86_64) instead of flushing them.

## What Changes

- New crate `rhizo-sched`: the run queue and sleep queue as plain data, a
  round-robin policy with a time slice, driven by explicit time values so it
  is deterministic and host-tested.
- Preemption: a timer interrupt ends a cell's time slice (AArch64 virtual
  timer through a GICv3; x86_64 local APIC timer). Interrupts are enabled only
  while cell code runs, so the kernel itself is never interrupted.
- New system calls: `sched_yield`, `nanosleep` and `clock_nanosleep`
  (relative, monotonic or realtime); a sleeping cell uses no CPU.
- Cell isolation tests: two cells using the same virtual addresses see their
  own memory, a cell cannot reach memory mapped only in another, a spinning
  cell does not starve the others, a faulting cell does not disturb them.
- Address-space identifiers: each live cell gets an ASID (AArch64) or PCID
  (x86_64, when the CPU has it), so a switch changes the root without a TLB
  flush; a cell's tag is flushed once, when the cell ends.
- Benchmark `cell.switch_ns`: two cells yielding to each other; the time per
  switch (system call in, scheduler, system call out).

### Out of scope

Multicore (one CPU still runs everything), priorities and fairness beyond
round robin, signals, threads inside a cell, suspend/resume (H4), fork (H5),
floating-point/SIMD state (cells run without it until H6, so there is no
state to leak between cells).

## Capabilities

### New Capabilities

- `scheduling`: several cells at once, preemption, yielding, sleeping.

### Modified Capabilities

- `cells`: isolation between cells.
- `linux-abi`: `sched_yield`, `nanosleep`, `clock_nanosleep`.

## Impact

- New crate `rhizo-sched` (no dependencies), added to the dependency table.
- Kernel: interrupt controller and timer per architecture (`arch::irq`), the
  scheduler loop replacing the run-to-completion loop of H2.
- `xtask`: QEMU `virt` gets `gic-version=3` (the GIC that Firecracker and
  Cloud Hypervisor expose), new test cells, `cell.switch_ns`.
