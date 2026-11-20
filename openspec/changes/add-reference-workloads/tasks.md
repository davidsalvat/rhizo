# Tasks

## 1. Measuring

- [x] 1.1 `rhizo_core::stats::Histogram` (D4); verify `cargo test -p rhizo-core stats` (quantiles against exact ones, property test)
- [x] 1.2 Boot options `bench=`, `cells=`, `secs=`, `llm=`; verify `cargo test -p rhizo-core options`
- [x] 1.3 `Scheduler::wake_due` (D4); verify `cargo test -p rhizo-sched`
- [x] 1.4 `kernel/src/prof.rs` and its hooks in `cell.rs` (D4, D5); verify `cargo xtask test --only ktest` on both architectures and no `cell.*` figure worse in CI beyond its threshold
- [x] 1.5 `rhizo-mark` lines stamped with the VMM's CPU time (D6); verify `cargo test -p xtask workloads`

## 2. Laptop workload

- [x] 2.1 `Host::wake` and its in-kernel test; verify `cargo xtask test --only ktest` (`a_woken_cell_runs_at_once`)
- [x] 2.2 `agent` cell, fake model server, `bench=laptop` (D1); verify `cargo test -p xtask hostnet` and `cargo xtask bench --workload laptop --vmm <default>` on both architectures

## 3. Server workload

- [x] 3.1 `worker` cell and `bench=server` with constant-rate host operations (D2); verify `cargo xtask bench --workload server` on both architectures: 10,000 cells, every worker exits with 0
- [x] 3.2 Out-of-memory push and its in-kernel test (D3); verify `memory_full_of_cells_is_refused_cleanly` passes on both architectures and on 4 CPUs (`--only ktest-smp`)

## 4. CI and gate

- [x] 4.1 `cargo xtask bench` runs both workloads, `--workload`, `--soak`, `--cells`, `--llm-delay-ms`; metrics where more is better (D7, D8); verify `cargo test -p xtask` and the CI benchmark job
- [x] 4.2 Weekly soak job; verify `actionlint` through `cargo xtask check`
- [x] 4.3 Stable workload metrics in the `ci` baseline from two CI runs, the others ungated with the reason; verify `cargo xtask bench --from <run> --env ci --check` on both runs

## 5. Documentation

- [x] 5.1 `bench/README.md`: the workloads, their metrics, how to run them, the shared-folder and vsock limitation; verify by reading it next to a CI run's output
