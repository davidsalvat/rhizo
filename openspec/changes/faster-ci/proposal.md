# Proposal

## Why

CI is the agents' feedback loop, and today every pull request runs
everything: about 9 minutes of wall clock whatever it touches (design,
*Baseline*). A pull request that changes one sentence of a design waits
for two kernels built on two architectures, every VM test and six minutes
of timing benchmarks. Those timings are gated at +100 % on shared runners
and still fail on noise: of the last four pull requests re-run on the same
commit, three had failed on a timing metric (`cell.fork_with_files_us`,
`workload.agent_cpython_model_us`, `workload.agent_cpython_turn_us`), one
of them a design-only change. A re-run costs another 9 minutes and an
agent's attention.

## What Changes

- **Path classes.** A first `changes` job classifies the diff against the
  merge base with a small script (`.github/scripts/classify.sh`, tested by
  `classify-test.sh`): *docs* (`docs/`, `openspec/`, `.agents/`, `*.md`,
  images), *host-tools* (`cli/` alone) or *code* (everything else). A docs
  change runs only a `docs` job: `openspec validate --specs --strict`, the
  changes it touches, relative Markdown links, and `docs/img/check.js`
  when images change. host-tools adds the quick checks and the control
  plane step. code runs everything. Pushes to main always run everything.
- **One required check.** An aggregate `ci-ok` job succeeds only when every
  job succeeded or was skipped by its class.
- **Fail fast.** A quick job (`cargo xtask check`: fmt, clippy on both
  architectures, host tests, dependency rules, actionlint) runs first; the
  heavy jobs wait for it.
- **Pull requests are gated on instruction counts only.** x86_64 and
  AArch64 each run `cargo xtask bench --icount-only --check` once (the
  counts are deterministic), in jobs of their own. Timing benchmarks, and
  the metrics with no instruction count (density percentiles, network
  round trips, wall times on Firecracker and Cloud Hypervisor, sizes), run
  with the full `ci` gate in a new `bench` workflow on every push to main
  and nightly; a failing gate there opens or updates an issue naming the
  metrics, the run and the commits since the last green timing run. The
  README's tables keep coming from that run's annotations.
- **Split the long jobs.** The AArch64 instruction counts and the x86_64
  Firecracker and Cloud Hypervisor tests leave the jobs they shared, so
  they run beside them.
- **Caching.** One composite setup action: the pinned toolchain,
  `Swatinem/rust-cache` keyed on the job, the compiler and `Cargo.lock`,
  saved only by main; the CPython build keyed on its recipe; downloaded
  tools keyed on their pinned list.
- **Flaky jobs recorded, never retried.** A `flaky` workflow notices a CI or
  bench run that succeeded on a re-run of the same commit and opens or
  updates a `flaky: <failure>` issue for each job that failed before.
- **Workflow rules.** AGENTS.md gets a "Working with many PRs" section
  (hotspots, stacked pull requests, merging main only on conflicts,
  reserved version numbers) and the CI behavior above; bench/README.md says
  where each gate runs.

## Impact

- Specs: `developer-feedback` (CI by change class, quick job, `ci-ok`,
  flaky jobs), `benchmarks` (where the gates run, the timing-gate issue),
  and, so they stay true for docs-only pull requests, `kernel-testing`
  (CI gate) and `architectures` (both architectures in CI).
- Code: `.github/workflows/{ci,bench,flaky}.yml`,
  `.github/actions/setup/action.yml`, `.github/scripts/`. No Rust changes:
  `--icount-only --check` already compares only the VMM that ran.
- What leaves pull requests: timing gates and metrics without an
  instruction count, now on main and nightly. A regression they catch
  lands on main first and is reported by the issue; the `bench` workflow
  can still be run by hand on a branch.
- Branch protection: none today; when added, it should require `ci-ok`.
