# Proposal

## Why

`add-cost-accounting` says how much of a workload's time the kernel spends
in each of its parts and system calls. It cannot say where cell code spends
its own time, which is most of it (93 % of the CPython cold start's
instructions). Optimizing the next bottleneck needs the functions, per cell
and next to the kernel's parts and system calls, in a form flame-graph
tools read.

## What Changes

- **Sampling** (kernel feature `sampling`, which includes `profile`): the
  PMU counts cycles of cell code only and interrupts every 100,000 of them
  (x86_64: general-purpose counter 2 and the local APIC's `LVTPC` on a new
  vector, 34; AArch64: PMUv3 event counter 1 and its overflow interrupt,
  a PPI the device tree names). Each sample records the cell and its program
  counter. Without a PMU the timer samples instead: its tick runs every
  100 µs while sampling, and a slice still lasts 10 ms.
- `rhizo-prof::samples`: the sample buffer (room reserved before sampling;
  a full buffer counts what it loses) and folding into counts per cell and
  address, tested on the host.
- `rhizo-core`: the PMU's interrupt from the device tree.
- `cargo xtask profile [--vmm] [--accel] [--out] [--per-cell]`: runs the
  cell loop, `fork_touch` and the CPython cold start sampled and accounted,
  names each address with the image's symbols (`nm`; CPython's from the
  interpreter as built, before stripping; addresses without a symbol fold by
  4 KiB page) and writes, per workload, folded stacks in nanoseconds:
  `<workload>.folded` (the cells' functions sharing the accounted user time
  by sample count, beside the kernel's parts) and `<workload>.calls.folded`
  (the system calls that took most time).

## Out of scope

- Kernel program counters. The kernel runs with interrupts masked, so an
  overflow interrupt arrives only once cell code runs again. Sampling the
  kernel needs a non-maskable interrupt on x86_64 (an NMI and its own stack,
  in the entry code another change is reworking for KPTI) and pseudo-NMIs
  (GICv3 priorities) or `FEAT_NMI` on AArch64. The kernel's time is covered
  exactly, per part and per system call, by the accounting instead.

## Capabilities

### Modified Capabilities

- `profiling` (added by `add-cost-accounting`): sampling and folded stacks.

## Impact

`crates/rhizo-prof` (`samples`), `crates/rhizo-core` (`fdt`), kernel
`arch/*/{pmu,irq,user}.rs`, `arch/x86_64/cpu.rs` (vector 34), `cost.rs`,
`cell.rs` (samples on `Trap::Sample` and timer ticks), `tests.rs`, the
`sampling` feature; `xtask` (`profile`, `cpython::interpreter`);
`bench/README.md`. Depends on `add-cost-accounting`.
