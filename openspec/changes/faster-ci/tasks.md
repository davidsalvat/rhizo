# Tasks

## 1. Measure

- [x] 1.1 Per-job and per-step durations of 5 green main runs and 6 pull-request runs, the re-runs and their causes (design *Baseline*); verify the tables against `gh api repos/davidsalvat/rhizo/actions/runs/<id>/jobs`

## 2. Classify and route

- [x] 2.1 `.github/scripts/classify.sh` and `classify-test.sh` (D1); verify `.github/scripts/classify-test.sh` and a dry run on the last 30 closed pull requests
- [x] 2.2 `.github/scripts/check-links.py` (D2); verify it passes on the repository and reports a broken link in a test file
- [x] 2.3 `.github/actions/setup` and `ci.yml`: `changes`, `docs`, `quick`, `policy`, `control-plane`, `ci`, `vmm`, `icount-x86_64`, `ci-arm64`, `icount-aarch64`, `miri`, `ci-ok` (D2–D4, D6); verify `actionlint` and `cargo xtask check`

## 3. Benchmarks on main, flaky jobs

- [x] 3.1 `bench.yml`: full benchmarks and gate on main, nightly and by hand, results published, the timing-gate issue (D5); verify `actionlint`
- [x] 3.2 `flaky.yml` (D7); verify `actionlint` and its queries on run 44916302134
- [x] 3.3 `bench/README.md` (where the gates run, where results come from) and AGENTS.md (CI classes, "Working with many PRs"); verify `python3 .github/scripts/check-links.py`
- [x] 3.4 `openspec validate faster-ci --strict`

## 4. Measure the result

- [ ] 4.1 Per-job times of this pull request's CI run against the baseline, and the classifier's verdict on it; verify from `gh api .../runs/<id>/jobs`
