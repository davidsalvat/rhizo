# Tasks

## 1. Kernel

- [x] 1.1 `Host::run_until` and the boot CPU's wait bounded by the run's limit (D1); verify `a_run_with_a_time_limit_ends_on_time` with `cargo xtask test --only ktest-smp` (it times out without the bound) and `--only ktest`
- [x] 1.2 The server loop, `agent_figures` and `wire_rtt` stop by `run_until` (D1); verify `cargo xtask bench --workload server --runs 1`

## 2. Server workload

- [x] 2.1 Templates frozen once asleep (D2); verify 35 consecutive `cargo xtask --arch x86_64 bench --workload server --cpus 4` boots without a hang or a worker left in the hour's sleep
- [x] 2.2 `--cpus` for the server VM, refused with `--check` and `--update-baseline` (D4); verify `cargo test -p xtask options_parse`

## 3. Gates

- [x] 3.1 The `-icount` gates and the density figures at 1 and 4 vCPUs; verify `cargo xtask bench --icount-only --env ci --check` and the server workload at `--cpus 1` and `--cpus 4`
- [x] 3.2 Everything else; verify `cargo xtask check`, `cargo xtask --arch x86_64 test` and `cargo xtask test`
