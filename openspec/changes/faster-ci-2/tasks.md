# Tasks

## 1. Measure

- [x] 1.1 Per-job and per-step durations of the code pull-request run of `fewer-kernel-builds` (44932285327), and the phases inside the AArch64 job and Miri from their logs (design *Baseline*); verify against `gh api repos/davidsalvat/rhizo/actions/runs/44932285327/jobs`
- [x] 1.2 Order independence of the in-kernel tests: `RHIZO_SHARDS=2`, `3` and `4` for `ktest` and `2` for `ktest-smp` on AArch64 (design D2); verify each shard's result and record the failing test

## 2. xtask

- [x] 2.1 `--part K/N` for `test` and `ci`, steps dealt by cost (`xtask/src/parts.rs`), lints and q35 steps in part 1 (D2); verify `cargo test -p xtask parts` and `the_parts_of_a_run_hold_every_step_once`
- [x] 2.2 `test --affected` (`xtask/src/affected.rs`, D4); verify `cargo test -p xtask affected`, and `cargo xtask test --affected --only boot` on this branch (it changes `xtask`, so every step is reachable) printing and running `boot`

## 3. Workflow

- [x] 3.1 `ci.yml`: heavy jobs need only `changes`; `quick` cancels the run on failure (D1); `ci-arm64` as a two-part matrix (D2); Miri in three processes (D3); verify `actionlint` and `cargo xtask check`
- [x] 3.2 AGENTS.md: the feedback loop table and the CI paragraph; verify by reading
- [x] 3.3 `openspec validate faster-ci-2 --strict`

## 4. Measure the result

- [x] 4.1 Per-job times and wall clock of this pull request's runs against the baseline (design *Result*); verify from `gh api .../runs/<id>/jobs`
- [x] 4.2 A failing quick check cancels the other jobs and fails `ci-ok`; verify on a run of this pull request with a deliberate format error, then reverted
