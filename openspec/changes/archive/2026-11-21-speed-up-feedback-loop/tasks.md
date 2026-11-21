## 1. Kernel
- [x] 1.1 `shard=I/N` boot option (host tests for parsing)
- [x] 1.2 `ktest::run` runs one share of the cases (host test: every case in exactly one share)
- [x] 1.3 The network card comes up in `cell::init`
- [x] 1.4 `cells_in_parallel_stay_isolated` warms up; `sleeping_cell_uses_no_cpu_and_wakes_on_time` sleeps 500 ms

## 2. Tooling
- [x] 2.1 `cargo xtask test` builds first, then runs steps in a CPU-weighted pool (`RHIZO_JOBS`), one at a time on Firecracker, Cloud Hypervisor and with `--gdb`
- [x] 2.2 `RHIZO_SHARDS=N` splits `ktest` and `ktest-smp`
- [x] 2.3 Benchmark gate fails on missing figures (host test)
- [x] 2.4 Cell benchmark VM with 256 MiB
- [x] 2.5 AGENTS.md documents `RHIZO_JOBS` and `RHIZO_SHARDS`

## 3. Verification
- [x] 3.1 `cargo xtask check`
- [x] 3.2 Full `cargo xtask --arch x86_64 test` passes in parallel (148 s against about 6 min)
- [x] 3.3 CI green on both architectures, with cell figures measured again on every VMM
