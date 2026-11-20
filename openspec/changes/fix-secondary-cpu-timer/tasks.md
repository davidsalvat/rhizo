# Tasks

## 1. Fix and test

- [x] 1.1 In-kernel test `timed_waits_last_as_long_on_every_cpu` with `cell::timed_wait` (D2); verify it fails on x86_64 before the fix with `cargo xtask --arch x86_64 test --only ktest-smp`
- [x] 1.2 Secondary CPUs set the local APIC timer divider (D1); verify `cargo xtask test --only ktest-smp` and `cargo xtask test` pass on both architectures

## 2. Metric

- [ ] 2.1 `timer_error_ns` in the `rhizo-smp` line and `cell.timer_error_us` (D3); verify `cargo test -p xtask smp_line` and the metric in the CI benchmark job
- [ ] 2.2 `bench/README.md` documents it; the `ci` baseline takes it and the lower laptop wake-ups from a CI run; verify `cargo xtask bench --from <run> --env ci --check`
