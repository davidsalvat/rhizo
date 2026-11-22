# Tasks

## 1. Reproduce

- [x] 1.1 Property test `many_domains_stay_within_one_stretched_slice` (three to six domains, slices up to 20 nominal ones mixed with short ones, 1 to 4 queues), sharing a helper with `stretched_slices_stay_within_one_slice`; verify it fails on `main` (`cargo test -p rhizo-sched stretch`, `PROPTEST_CASES=2000`)
- [x] 1.2 Measure the worst distance on `main` and with the fix in a scratch copy of `Fair` (not committed), and search a model of the pick for a counterexample (design, evidence)

## 2. Fix the pick

- [x] 2.1 Finish with the longest slice charged so far (`Fair::longest`, `Fair::lengthen`); the fallback when no domain is eligible out of the pick loop (`Fair::least`); verify `cargo test -p rhizo-sched`, and the two stretched tests with 30,000 cases under two other seeds
- [x] 2.2 Write the bound's proof (design, D1)

## 3. Measure the domain test as charged

- [x] 3.1 `rhizo-sched` feature `ktest` (turned on by the kernel's `ktest`): `Fair::domain_charged_ns`; `Host::charged_ns` reads the domain's charge; `Fair::charged_ns` goes (D2)
- [x] 3.2 `domains_share_by_weight` over at least 2 s and 20 longest slices, band unchanged; verify `ktest-smp` on both architectures, and eight runs in a row of both share tests (scratch) under 10 and 16 busy loops on x86_64 and 12 on AArch64 (design, verification under load)

## 4. Gates

- [x] 4.1 Verify `openspec validate fix-fair-bound --strict` and `cargo xtask check`
- [x] 4.2 Verify `cargo xtask ci` on both architectures
- [x] 4.3 No benchmark regression: verify `cargo xtask bench --icount-only` on both architectures (`cell.switch_insns` 872 and 824, unchanged; `cell.switch_domains_insns` 1373 to 1342 and 1287 to 1266; x86_64 and AArch64)
