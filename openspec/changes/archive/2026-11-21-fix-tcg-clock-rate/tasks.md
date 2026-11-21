# Tasks

## 1. Reproduce

- [x] 1.1 `rhizo up --arch x86_64 --accel tcg --cpus 4`, `rhizo chat` against a fake model holding each answer 65 s, then 3 s; verify the 65 s turns report about 200 ms of delivery and a model time under 65 s, the 3 s turns 10–15 ms
- [x] 1.2 Cells' clock against the host's over 60 s with no traffic, and `tsc_hz` against the host's TSC frequency; verify the cell's clock is slow by the same fraction

## 2. Fix

- [x] 2.1 Tight readings at both ends of the APIC timing, a warm-up batch, a short PIT count first (D1); verify the 65 s turns report a model time of at least 65 s and the same delivery as 3 s turns
- [x] 2.2 In-kernel test `the_clock_runs_at_the_apic_timer_s_rate` (D2); verify `cargo xtask --arch x86_64 test --only ktest` on `qemu-microvm` and `qemu-q35`, and that it fails with the old calibration on both

## 3. Gates

- [x] 3.1 No benchmark moves: verify `cargo xtask --arch x86_64 bench --icount-only` against `bench/baseline.json`
- [x] 3.2 Verify `cargo xtask check` and `cargo xtask --arch x86_64 test`
