# Tasks

## 1. Scheduler

- [x] 1.1 `rhizo_sched::Scheduler` with one queue per CPU, arrival numbers, `place`, `requeue`, `next` (own queue, then steal the oldest), `next_oldest` (D2); host tests, and property tests of interleaved CPUs (nothing lost, no idle CPU with work, never early, nobody starves, one CPU equals a single round robin); verify `cargo test -p rhizo-sched`

## 2. Kernel

- [x] 2.1 Per-CPU arch state (loaded root, FP trap bit, FS base), per-CPU FP/SIMD set-up and SSE/PCIDs on secondary CPUs, tick start/stop, `leave_user`, `TLB_FLUSH_IS_BROADCAST` (D4, D5); verify `cargo xtask test` on both architectures
- [x] 2.2 `Host` with `Shared` behind one lock, `run` on every CPU, `step` on one, idle bits and wake-ups, ended cells collected (D3); verify `cargo xtask test --only ktest-smp` on both architectures
- [x] 2.3 Per-cell clean set for TLB flushes (D5); eager FP save and root release when giving a cell back (D4); verify `cargo xtask --arch x86_64 test --only ktest-smp`
- [x] 2.4 `SHARING` lock for cells that share memory (D6); verify `children_outlive_their_parent_and_frames_return` under `ktest-smp`
- [x] 2.5 In-kernel tests: cells on every CPU, busy CPUs do not starve a cell, programs in parallel stay isolated, suspend between parallel runs; `spin N`; verify `cargo xtask test` on both architectures, `ktest-smp` three times in a row

- [x] 2.6 Heap window for large allocations when no contiguous run is left (D7); in-kernel test with every other frame taken; `memory_pressure_fails_cells_not_the_kernel` floods with 16 MiB cells so memory overflows whatever the order cells run in on several CPUs; verify `cargo xtask test` and `ktest-smp` on both architectures

- [x] 2.7 Merge `main` (sockets, virtio-net, CPython): waiting set per CPU, counter seen by a cell's last call, network card polled by the boot CPU (D9); verify `cargo xtask test` and `ktest-smp` (repeated) on both architectures

## 3. Benchmarks and documentation

- [x] 3.1 `rhizo-smp` line of `bench-cell` with several CPUs, `cell.parallel_us`, `cell.scaling_pct`, `cell.ipi_wake_us` parsed by `xtask` with a unit test, `bench/README.md` (D8); verify `cargo xtask --arch x86_64 bench --runs 1 --vmm qemu-microvm --no-reference` locally
- [x] 3.2 `AGENTS.md`, `docs/security.md`; `cargo xtask ci` on both architectures; CI green with the bench gate
