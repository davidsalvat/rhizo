# Tasks

## 1. Baseline

- [x] 1.1 Before the change, on both architectures: the in-kernel test list of `ktest` and `ktest-smp` (`RHIZO_JOBS=1 cargo xtask --arch <arch> test --only <step>`), and the `-icount` benchmarks (`cargo xtask --arch <arch> bench --icount-only`); verify both runs pass

## 2. Split

- [x] 2.1 `kernel/src/cell.rs` into `kernel/src/cell/` by concern, items moved unchanged, `pub(super)` where a sibling needs them (D1); verify `cargo clippy -p rhizo-kernel -- -D warnings` for the plain, `ktest`, `bench-cell`, `bench-cell,profile`, `bench-cell,sampling`, `control`, `ktest,packages`, `bench-cell,packages`, `ktest,node` and `bench-cell,node` sets on both targets
- [x] 2.2 `kernel/src/tests.rs` into areas under `kernel/src/tests/`, each with its `ALL`, concatenated in `AREAS` order (D2); verify the same lint sets, and that the concatenated names equal the old `suite!` list
- [x] 2.3 `AGENTS.md` (code map, hotspots) and `.agents/skills/kernel-test/SKILL.md`; verify by reading

## 3. Verify

- [ ] 3.1 The same tests by name and order; verify the `ktest` and `ktest-smp` lists after the change against 1.1 on both architectures
- [ ] 3.2 The same code; verify the release plain and `bench-cell` kernels function by function against the baseline on both architectures, and the `-icount` benchmarks against 1.1
- [ ] 3.3 `cargo xtask ci` and `cargo xtask --arch x86_64 ci` pass; `openspec validate split-hotspot-files --strict` passes
