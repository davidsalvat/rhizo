# Tasks

## 1. Reproduce

- [x] 1.1 Tabulate `density.*_p99_us` and the breakdown's `resume_ns` over main's `bench` runs before and after `c6c8ca4` (design, evidence); verify the step appears on every runner type and the p50 does not move
- [x] 1.2 Time `Tree::end` and `Tree::rekey` with 10,004 cells in a scratch host benchmark (not committed); verify ends cost O(n) each (88 ms in all)
- [x] 1.3 Run the server workload under `-icount` on `5a23f6d` and `origin/main`; verify the p99 goes from 14 ms to 159 ms

## 2. Fix

- [x] 2.1 Gone nodes and compaction in `rhizo_cap::Tree` (D1, D3, D4); verify `cargo test -p rhizo-cap` (the new `cells_that_left_leave_no_trace`, the property tests unchanged)
- [x] 2.2 `rekey` appends and buries, rotating within 16 nodes or without memory (D2); verify `cargo test -p rhizo-cap` (`rekey_keeps_the_place`, `failed_allocations_change_nothing`)
- [x] 2.3 Verify the scratch benchmark of 1.2: ends 73 ns each, `rekey` 0.24 µs

## 3. Gates

- [x] 3.1 Verify the server workload under `-icount` gives the p99 and `resume_ns` of `5a23f6d` again (design, evidence)
- [x] 3.2 Verify `cargo xtask check`, and `cargo xtask test` on both architectures
- [x] 3.3 Instruction counts within the gate: verify `cargo xtask --arch x86_64 bench --icount-only --env ci --check` (fork and resume +0.5 % and +1.3 %, design)
- [x] 3.4 PR #138's weighted scheduler measured with and without this change under `-icount` (design, evidence)
- [ ] 3.5 After merge, the `bench` workflow on main passes `density.sched_p99_us` and `density.wake_p99_us` on every VMM
