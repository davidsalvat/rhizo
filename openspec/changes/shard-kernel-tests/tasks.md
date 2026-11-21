# Tasks

## 1. Root cause

- [x] 1.1 Reproduce: `RHIZO_SHARDS=2 cargo xtask --arch aarch64 test --only ktest` fails part 2 at `kernel/src/tests/dynamic.rs:46` ("nothing leaked", 1038 against 1040); verify the failure
- [x] 1.2 Measure the card's state at both measures of the failing test (design D1); verify `wire_busy()` is true before and false after

## 2. Fix

- [x] 2.1 `settle_wire()` in `memory_balance()`, and in place of the loops of the CPython agent and package tests (D2); verify `RHIZO_SHARDS=2 cargo xtask --arch aarch64 test --only ktest` passes
- [x] 2.2 A split step's `reproduce` command carries `RHIZO_SHARDS=N`; verify `cargo test -p xtask a_split_step_is_reproduced_with_the_same_split`
- [x] 2.3 `docs/security.md`: the H13 tests are in `kernel/src/tests/isolation.rs`; verify by reading

## 3. Shares dealt by cost

- [x] 3.1 Per-test durations on AArch64, one CPU and four (D3); verify by one live-output run of each step
- [x] 3.2 `TestCase::cost`, `ktest_case!(name, cost)`, the deal by rank, costs on the 19 tests of 3 s or more (D3); verify `cargo test -p rhizo-core ktest` and the two VMs of `RHIZO_SHARDS=2` on AArch64 within 10 % of each other

## 4. Every split

- [x] 4.1 `ktest` and `ktest-smp` with `RHIZO_SHARDS` 1 to 4 on AArch64 and x86_64 (D4); verify every VM passes

## 5. CI

- [x] 5.1 `ci-arm64` with `RHIZO_SHARDS=2` (D5), AGENTS.md; verify `actionlint` and `cargo xtask check`
- [x] 5.2 `cargo xtask ci` on both architectures; verify it passes
- [x] 5.3 Per-job times of this pull request's runs against the baseline (design *Result*); verify from `gh api repos/davidsalvat/rhizo/actions/runs/<id>/jobs`, and that both VMs of `ktest` and of `ktest-smp` passed in the job logs (that they hold every test once is `shards_run_every_case_exactly_once`)
- [x] 5.4 `openspec validate shard-kernel-tests --strict`
