# Proposal

## Why

The laptop reference workload (`add-reference-workloads`) found that on
x86_64 the CPUs other than the boot CPU never set the divider of their
local APIC timer: it keeps its reset value, so every count is shorter than
the 16 ns the kernel assumes. On those CPUs each time slice and each timed
wait of the kernel is several times too short (a 20 ms wait ends after
about 1.3 ms on QEMU): cells there are preempted far more often than on the
boot CPU, and an idle CPU wakes up many more times than it should
(`laptop.idle_wakeups_per_s` about 880 in CI on 2 vCPUs, against about 200
on AArch64). No test noticed, because nothing compared a secondary CPU's
timer with the counter.

## What Changes

- x86_64 secondary CPUs set the local APIC timer divider to the boot CPU's
  (divide by 16) when they enable their local APIC.
- A requirement that timers count the same time on every CPU, checked by an
  in-kernel test that times a 20 ms wait on each CPU's own timer against
  the counter (it fails without the fix on x86_64 with 4 vCPUs).
- A metric, `cell.timer_error_us`: the distance from 10 ms of a 10 ms timed
  wait on the secondary CPUs (the worst CPU), measured by the 4-vCPU
  benchmark boot.

### Out of scope

Tickless idle with several CPUs (an idle CPU still re-checks every slice
while any cell sleeps): its own change, `add-tickless-idle`.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `multi-core`: timers count the same time on every CPU.
- `benchmarks`: the timer accuracy metric.

## Impact

`kernel/src/arch/x86_64/irq.rs` (one register write), `kernel/src/cell.rs`
(`timed_wait`, the `bench-cell` figure), `kernel/src/tests.rs`,
`xtask/src/bench.rs`, `bench/README.md`, `bench/baseline.json` (the laptop
wake-ups improve; the new metric enters the `ci` baseline).
