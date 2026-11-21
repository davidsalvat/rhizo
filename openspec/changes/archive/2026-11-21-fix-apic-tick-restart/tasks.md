# Tasks

## 1. Timer

- [x] 1.1 `tick()` writes the initial count before the periodic mode, through `tick_writes`; `wait_ns` through `wait_writes` (D1); verify `cargo xtask --arch x86_64 test --only ktest`
- [x] 1.2 In-kernel test `the_timer_is_never_periodic_with_a_short_count`, failing with the old order (D1); verify it fails with the writes swapped back and passes with them as in 1.1

## 2. Evidence

- [x] 2.1 No stall in 20 runs in a row of `cargo xtask --arch x86_64 test --only ktest-smp`, and `cargo xtask --arch aarch64 test --only ktest-smp` passes; verify both commands
