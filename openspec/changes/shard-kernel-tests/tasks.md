# Tasks

## 1. Root cause

- [x] 1.1 Reproduce: `RHIZO_SHARDS=2 cargo xtask --arch aarch64 test --only ktest` fails part 2 at `kernel/src/tests/dynamic.rs:46` ("nothing leaked", 1038 against 1040); verify the failure
- [x] 1.2 Measure the card's state at both measures of the failing test (design D1); verify `wire_busy()` is true before and false after

## 2. Fix

- [x] 2.1 `settle_wire()` in `memory_balance()`, and in place of the loops of the CPython agent and package tests (D2); verify `RHIZO_SHARDS=2 cargo xtask --arch aarch64 test --only ktest` passes
- [x] 2.2 A split step's `reproduce` command carries `RHIZO_SHARDS=N`; verify `cargo test -p xtask a_split_step_is_reproduced_with_the_same_split`
- [x] 2.3 `docs/security.md`: the H13 tests are in `kernel/src/tests/isolation.rs`; verify by reading

## 3. Every split

- [ ] 3.1 `ktest` and `ktest-smp` with `RHIZO_SHARDS` 1 to 4 on AArch64 and x86_64 (D3); verify every VM passes

## 4. CI

- [ ] 4.1 `ci-arm64` with `RHIZO_SHARDS=2` (D4), AGENTS.md; verify `actionlint` and `cargo xtask check`
- [ ] 4.2 `cargo xtask ci` on both architectures; verify it passes
- [ ] 4.3 Per-job times of this pull request's runs against the baseline (design *Result*); verify from `gh api repos/davidsalvat/rhizo/actions/runs/<id>/jobs`, and that every test ran once per step in the job logs
- [ ] 4.4 `openspec validate shard-kernel-tests --strict`
