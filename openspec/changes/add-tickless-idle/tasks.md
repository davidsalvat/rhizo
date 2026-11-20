# Tasks

## 1. Which idle CPU holds the timer

- [x] 1.1 `rhizo_sched::IdleTimer` with unit and property tests (D1); verify `cargo test -p rhizo-sched idle`, and the property test fails when `leave` or `cover` does nothing

## 2. Kernel

- [x] 2.1 In-kernel test `idle_cpus_wake_only_for_deadlines` (D4); verify it fails before the change with `cargo xtask --arch x86_64 test --only ktest-smp`
- [x] 2.2 `take` uses `IdleTimer`, a done CPU counts as idle, `wait` stops the tick without a deadline, the timer is reset between runs (D1, D2); verify `cargo xtask test --only ktest-smp` and `cargo xtask test` on both architectures
- [x] 2.3 AArch64 `wait_ns` restarts the tick whatever ended the wait, and long waits fit `CNTV_TVAL_EL0` (D2); verify `cargo xtask --arch aarch64 test --only ktest-smp`

## 3. Metric

- [ ] 3.1 `laptop.idle_wakeups_per_s` drops in the CI benchmark job; the `ci` baseline takes it from two CI runs that agree within the threshold, with the host CPU; `bench/README.md` says what still wakes idle CPUs; verify `cargo xtask bench --from <run> --env ci --check` on both runs
