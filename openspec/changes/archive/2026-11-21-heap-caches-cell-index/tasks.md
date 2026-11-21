# Tasks

## 0. Profile

- [x] 0.1 Heap allocations per spawn, fork and system call, heap lock waits on 4 CPUs, live blocks by size in the server and laptop workloads, cost of the cell lookups at 10,000 cells (D0); verify the numbers are in `design.md`

## 1. Heap

- [x] 1.1 Classes between the powers of two, the power-of-two classes unchanged for larger alignments (D1); verify `cargo test -p rhizo-mm heap` and `cargo +nightly miri test -p rhizo-mm -- heap::`

## 2. Scheduler

- [x] 2.1 Sleepers' hints, written by `sleep`, used by `remove`, `find` and `find_mut` (D2); verify `cargo test -p rhizo-sched`, the property test against a search of every place included
- [x] 2.2 In-kernel test of host operations among hundreds of sleepers; verify `cargo xtask test --only ktest` and `--only ktest-smp` on both architectures

## 3. Benchmarks

- [x] 3.1 Kills timed in the server workload; `density.suspend_us`, `density.fork_us`, `density.kill_us` (D3); verify `cargo test -p xtask server_` and `cargo xtask bench --workload server`, before and after
- [x] 3.2 Every `-icount` gate, `cell.switch_insns` unchanged; verify `cargo xtask bench --icount-only` on both architectures, before and after
- [x] 3.3 `bench/README.md` lists the new metrics; verify by reading

## 4. CI

- [x] 4.1 Everything CI runs; verify `cargo xtask check`, `cargo xtask test` on both architectures, and CI (pull request #108, run 44916198844, green)
