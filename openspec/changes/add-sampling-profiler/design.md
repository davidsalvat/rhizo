# Design

## D1. Cell code only, through ordinary interrupts

The kernel runs with interrupts masked and takes them only while cell code
runs or a CPU idles (spec `scheduling`). A sampling interrupt raised while
the kernel runs would be taken at the next return to cell code and blamed on
the cell's next instruction. So the sampling counter counts cycles of cell
code only (x86_64 `USR` without `OS`; AArch64 `PMEVTYPER.P` set): it can
only overflow while cell code runs, and the interrupt comes at once, with
the cell's program counter in the saved registers. `enter_user` returns
`Trap::Sample`, the host records the sample and re-enters the cell; the time
slice is not affected.

The kernel's own time is not sampled: the accounting of
`add-cost-accounting` measures it exactly, per part and per system call,
and the folded stacks put both side by side. Sampling kernel addresses
needs an NMI on x86_64 (its own IST stack, a handler that touches neither
GS nor kernel locks, and care with KPTI, which another change is adding to
the same entry code) and pseudo-NMIs or `FEAT_NMI` on AArch64; it is left
for when the entry code settles.

Alternative: count in both modes and attribute an overflow taken late to
the kernel part open at the time (rejected: the part is already known
exactly from the accounting, and the user samples would be skewed).

## D2. The counters

x86_64: general-purpose counter 2 (counters 0 and 1 count instructions and
cycles for the benchmarks), unhalted cycles, `USR | INT | EN`, preloaded with
minus the period (Intel sign-extends the 32 bits written; AMD takes 48),
global enable bit 2 and overflow clear where the PMU has them (Intel
version 2, AMD `PerfMonV2`); the local APIC's `LVTPC` delivers vector 34
(fixed). Intel masks `LVTPC` when it delivers the interrupt, so each sample
re-arms the counter, clears the overflow and unmasks it.

AArch64: event counter 1, `CPU_CYCLES` (0x11), its overflow interrupt
enabled (`PMINTENSET`), the interrupt a PPI the device tree's
`arm,armv8-pmuv3` node names (QEMU, Firecracker and Cloud Hypervisor: PPI 7,
INTID 23), enabled on the CPU's redistributor like the timer's. The counter
is 64-bit when `PMCR.LP` is set (PMUv3p5), else 32-bit, whichever the PMU
counts instructions with.

An overflow can still be lost while the CPU idles with the interrupt
pending (waking acknowledges it); the host re-arms the counter after each
idle period.

## D3. The timer where there is no PMU

QEMU emulates no x86 PMU, and a hypervisor may give the guest none. Then
the periodic tick runs every 100 µs while sampling (`irq::set_tick_ns`) and
each tick taken from cell code is a sample; the host preempts a cell after
101 ticks instead of 2, so a slice still lasts 10 ms. Without `sampling`,
`prof::ticks_per_slice` is the constant 2 and the code is the same as
before.

## D4. Buffers and folding

`rhizo_prof::samples::Samples` holds a `Vec` whose room is reserved when
sampling starts (65,536 samples), so recording a sample never allocates; a
full buffer counts what it loses. At the end the samples are sorted and
folded into `(cell, address, count)` lines.

## D5. Folded stacks in time

`cargo xtask profile` runs the `bench-cell,sampling` kernel, whose
breakdowns run each workload five times with sampling on. Each sample is
worth the workload's accounted `user` time divided by its number of
samples, so the cells' functions and the kernel's parts share one unit
(nanoseconds) and add up to the workload's time. Addresses are named with
`nm -n -C` (binutils, already needed by the C cells' build): the Rust cells
are stripped, so their addresses fold by 4 KiB page; CPython's come from
the interpreter as built, before the copy the cell runs is stripped. The
output is the folded format `flamegraph.pl`, `inferno` and speedscope read;
no new dependency.
