# Proposal

## Why

The project enters a "find the limits" phase: small versions of the real
workloads (servers with thousands of cells, laptops with a few dozen agents)
will be measured in detail, and every bottleneck has to show up as a metric
before anyone optimizes it. Today the benchmarks say how long an operation
takes, not where the time goes. An ad-hoc profile of the CPython cold start
(17.6 ms) found about 88 % in user code, 0.6 ms spawning and loading, 0.4 ms
in system calls, and about 1.1 ms nobody could attribute (scheduler,
interrupts, teardown).

Timings on shared runners are also noisy (two runs of identical code differ
by tens of percent), so the CI gate only catches gross regressions. Retired
instructions barely move from run to run, and under QEMU's instruction
counting (`-icount`) they do not move at all.

## What Changes

- **Cost accounting** (kernel feature `profile`): hooks at the kernel's
  transitions charge each CPU's time to one of thirteen parts (cell code,
  system calls, page faults, copy-on-write, the frame allocator, copies, the
  network, the scheduler, idle, lock waits, cell operations, teardown, the
  rest) and to each system call by number. The parts are exclusive and add
  up to the time accounted. The logic lives in a new library crate,
  `rhizo-prof`, tested on the host. Without the feature the hooks compile to
  nothing.
- **Breakdowns** of the cell benchmark loop, a fork writing 50 MiB, the
  CPython cold start and sixteen CPU-bound cells on four CPUs, published as
  `breakdown.<workload>.<part>_us` (the cell's time split into its own code
  and the kernel's entry and exit path).
- **Lock contention**: every kernel spinlock counts the times a CPU waited
  for it and the spins meanwhile, on the waiting path only; published as
  `lock.<name>_waits` for the four-CPU workload, whose waiting time is the
  `lock` part of its breakdown.
- **Performance counters** behind the `arch` interface (`pmu`: Intel's
  architectural performance monitoring and AMD's core counters on x86_64,
  PMUv3 on AArch64). Where the guest has them, every timed benchmark is run
  again counting retired instructions, then core cycles:
  `<metric>_insns/rhizo/<vmm>` and `<metric>_cycles/rhizo/<vmm>`.
- **Instruction counts under `-icount`**: `cargo xtask bench` also runs the
  benchmarks and breakdowns under QEMU with `-icount shift=0`, where the
  guest's clock counts its instructions: `<metric>_insns/rhizo/<qemu>-icount`,
  exact on every run and gated at 2 % in CI.
- New metrics: `cell.syscall_ns` (a `getpid` round trip) and
  `cell.cow_fault_ns` (the kernel's side of a copy-on-write fault of a
  4 KiB page).
- Copy-on-write copies a page in one call (`PhysicalMemory::copy`, one
  `memcpy` in the kernel) instead of eight 512-byte round trips through a
  buffer, so a copy is one accounted event (and is faster).

## Out of scope

- The sampling profiler (PMU overflow interrupts, folded stacks per cell and
  per system call): the next change, `add-sampling-profiler`, builds on the
  `pmu` interface and the accounting added here.
- Splitting the kernel entry and exit path from cell code exactly: that
  needs timestamps in the entry assembly; here it is estimated from a cell
  that does nothing but system calls.

## Capabilities

### New Capabilities

- `profiling`: cost accounting, breakdowns, lock contention, performance
  counters, instruction counts and their gate.

## Impact

New crate `crates/rhizo-prof`; `rhizo-core` (`SpinLock` contention),
`rhizo-hal` (`PhysicalMemory::copy`), `rhizo-mm` (copy-on-write copies),
`rhizo-abi` (call number, name and socket class for reports); kernel
`prof.rs`, hooks in `cell.rs`, `memory.rs`, `sync.rs`, `arch/*/pmu.rs`, the
`profile` feature and the benchmark passes; `cells` (`yielder getpid`);
`xtask` (`profile.rs`, `--accel icount`, `--no-icount`, gate thresholds);
`bench/README.md`, `bench/baseline.json`, `AGENTS.md`.
