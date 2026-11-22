# Tasks

## 1. Reproduce

- [x] 1.1 Repeat the test's setup 12 to 30 times in `ktest-smp` on x86_64 with 6 and 10 busy loops on the host (scratch, not committed); record each root's `cpu_ns`, the scheduler's charge, the slices and the longest slice; verify that runs leave 70–80 % (65.6 %, 66.4 %, 82.0 %) and that no run's error exceeds its longest slice (design, evidence)
- [x] 1.2 Explore `Fair` on the host with slices of random length (scratch, not committed); verify two domains stay within one longest slice (design, evidence)

## 2. Fix the measurement

- [x] 2.1 `Fair::charged_ns` and the property test `stretched_slices_stay_within_one_slice`; verify `cargo test -p rhizo-sched` (and with `PROPTEST_CASES=5000`)
- [x] 2.2 `Host::charged_ns` (in-kernel tests only) and `root_weights_share_a_cpu` measured as charged over at least 1 s and 20 longest slices (D1); verify 25 runs in a row of the test (scratch) under 10 busy loops in x86_64 `ktest-smp` (73.1 to 75.4 %), and under 6 busy loops with every AArch64 step at once (`cargo xtask test`)

## 3. Gates

- [x] 3.1 Verify `openspec validate fix-share-test --strict` and `cargo xtask check`
- [x] 3.2 Verify `cargo xtask ci` on both architectures
- [x] 3.3 No benchmark regression: verify `cargo xtask bench --icount-only` on both architectures (`cell.switch_insns` 873 and 826, baseline 880 and 832; `cell.switch_domains_insns` 1374 and 1290, baseline 1373 and 1286; x86_64 and AArch64)
