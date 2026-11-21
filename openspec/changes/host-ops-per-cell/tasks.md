# Tasks

## 1. Scheduler

- [ ] 1.1 Keys kept next to entries; `remove`, `find`, `find_mut` by key; `pop` (D4); verify `cargo test -p rhizo-sched`

## 2. Kernel

- [ ] 2.1 `Shared::on_cpu`, `CLAIM` and the claim slot; `take_cell`, `with_live`, `admit`; every host operation through them; `stop` gets `&mut Host` (D1, D2, D3); verify the in-kernel `mixed_host_operations_during_a_run_are_invisible` with `ktest-smp` on both architectures
- [ ] 2.2 `progress` and `Host::watch`; verify the in-kernel `host_operations_leave_other_cells_running` with `ktest-smp` on both architectures
- [ ] 2.3 Existing isolation, fork and snapshot tests unchanged; verify `cargo xtask test` and `cargo xtask --arch x86_64 test`, and `ktest-smp` ten times on each architecture
- [ ] 2.4 The server workload operates from the stop condition (D1); verify `cargo xtask bench --workload server` at 10,000 cells, before and after

## 3. Benchmarks

- [ ] 3.1 `cell.host_op_stall_us` and `cell.host_op_stall_stopped_us` in the `rhizo-smp` line and `xtask` (D6); verify `cargo test -p xtask smp_line` and a `bench-cell` run on 4 vCPUs on both architectures
- [ ] 3.2 Every `-icount` gate; verify `cargo xtask bench --icount-only --env ci --check` on x86_64 and `--env ci-arm` on AArch64
- [ ] 3.3 `bench/README.md` lists the new metrics; verify by reading

## 4. CI

- [ ] 4.1 Everything CI runs; verify `cargo xtask check` and CI
