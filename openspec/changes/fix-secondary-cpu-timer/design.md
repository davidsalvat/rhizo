# Design

## D1. Set the divider when a secondary CPU enables its local APIC

`irq::init` (boot CPU) writes the divide configuration register (divide by
16: `NS_PER_COUNT = 16` at the 1 GHz timer of KVM and QEMU) before it
starts the tick; `irq::init_cpu` (every other CPU) did not, and the register
is per CPU. `init_cpu` now writes the same value, next to the spurious
vector and the masked timer it already sets.

Alternatives: TSC-deadline mode on every CPU (no divider at all, but a
larger change of the timer code, and not offered by every VMM);
calibrating each CPU's timer against the TSC (slow at boot, and the
divider would still differ between CPUs).

## D2. Test: a timed wait on each CPU's own timer, against the counter

`cell::timed_wait(ns)` waits on the calling CPU's timer exactly as an idle
CPU does (`irq::wait_ns`) and returns how long it lasted by the counter; on
a secondary CPU it stops the tick again afterwards, as such a CPU idles
between runs. `timed_waits_last_as_long_on_every_cpu` runs it on every CPU
through `smp::post`, takes the longest of three tries (another interrupt may
end a wait early) and requires from 90 % to 10 times the 20 ms asked.
Without the fix it fails on x86_64 with 4 vCPUs (`CPU 1: a 20000000 ns wait
took 1345681 ns`).

Alternatives: counting ticks over a busy cell's run on a secondary CPU
(needs a tick counter only for the test, and depends on scheduling);
comparing slice lengths through a cell's `clock_gettime` (indirect and
noisy).

## D3. Metric: distance from 10 ms of a 10 ms timed wait

The 4-vCPU `bench-cell` boot adds `timer_error_ns` to its `rhizo-smp`
line: on each secondary CPU the median of five 10 ms `timed_wait`s, its
distance from 10 ms, the worst CPU's. `xtask` reports it as
`cell.timer_error_us`. A distance rather than the time waited, so that
lower is better whichever way a timer is wrong.
