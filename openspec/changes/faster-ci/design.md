# Design

## Baseline (before the change)

Five green CI runs on main (44915910240, 44917122283, 44918581126,
44919980424, 44921204895) and six green pull-request runs (44918728972,
44920168317, 44920339201, 44921045383, 44923231348, 44923467834), all of
2026-11-21, from `gh api repos/davidsalvat/rhizo/actions/runs/<id>/jobs`.
Job time is from start to completion; wall clock from the first job's
start to the last job's end.

| Job | main, median (range) | PR, median (range) |
|---|---|---|
| VMM boots and benchmark gate (x86_64, KVM) | 8.9 min (8.6–8.9) | 9.0 min (8.8–9.4) |
| AArch64 fmt, clippy, tests, instruction counts | 8.3 min (7.9–8.4) | 8.3 min (8.1–8.6) |
| x86_64 fmt, clippy, tests | 4.2 min (4.0–4.4) | 4.5 min (4.0–5.1) |
| Miri on the heap allocator | 3.7 min (3.6–5.5) | 3.6 min (3.0–5.3) |
| dependency policy, unused dependencies, dependency rules | 2.3 min (1.9–2.6) | 2.0 min (1.8–2.1) |
| **Wall clock** | **8.9 min** (8.6–9.0) | **9.0 min** (8.8–9.4) |

The docs-only pull requests among them (#115, a design) took the same
9.4 minutes as code.

Steps that take more than 3 s, median over the 11 runs:

| Job | Step | Median |
|---|---|---|
| bench | Benchmarks and regression gate (`bench --env ci --check`) | 371 s (360–378) |
| bench | In-kernel tests on Firecracker / 4 CPUs on Firecracker / on Cloud Hypervisor | 23 s / 17 s / 16 s |
| bench | Boot on Firecracker / Cloud Hypervisor | 19 s / 3 s |
| bench | Node.js tests on Firecracker | 10 s |
| bench | Install QEMU / CRIU / toolchain | 26 s / 14 s / 10 s |
| AArch64 | Run all checks (`cargo xtask --arch aarch64 ci`) | 306 s (291–315) |
| AArch64 | Instruction counts and regression gate | 152 s (151–153) |
| x86_64 | Run all checks (`cargo xtask --arch x86_64 ci`) | 212 s (177–246) |
| Miri | heap tests under Miri | 204 s (169–318) |
| quality | `cargo xtask check` | 63 s (47–89) |
| quality | cargo-deny | 23 s (+13 s building its container) |

Inside `cargo xtask --arch aarch64 ci` (run 44921204895): format and
clippy for every feature set 25 s, the differential check 16 s, building
the host tests and every kernel 93 s, then the VM steps at once for 173 s
(in-kernel tests 93 s, on 4 CPUs 82 s, Node.js 24 s, PyPI packages 19 s,
control plane 33 s, host tests 67 s, all sharing the runner's 4 CPUs).
x86_64 is the same shape with KVM: 94 s of builds, 45 s of VM steps. In
the timing benchmark step, the x86_64 instruction counts take 14 s once
the kernels are built; the rest is timings over 20 boots per VMM, the
workloads and the comparisons.

Re-runs: of the last four pull-request runs with `run_attempt > 1`
(44916302134, 44875087298, 44869367542, 44874913229), three failed first
on a timing metric of the `ci` gate and passed on the same commit; the
fourth failed an in-kernel test and stayed red.

## D1. Classify by path, in a script

The `changes` job checks out the full history, lists the changed paths
with `git diff --name-only <base>...<head>` (the three dots diff from the
merge base) and pipes them to `.github/scripts/classify.sh`, which prints
the widest class of any path:

| Class | Paths |
|---|---|
| docs | `docs/**` (with `docs/img/**` and its generators), `openspec/**`, `.agents/**`, `*.md` anywhere, `bench/*.svg` (README charts) |
| host-tools | `cli/**`, alone or with docs |
| code | everything else: `crates/**`, `kernel/**`, `xtask/**`, `cells/**`, `.github/**`, `Cargo.*`, `rust-toolchain.toml`, `bench/baseline.json`, `deny.toml`... and an empty list |

No code reads a Markdown file at build or test time (`xtask bench
--readme` writes the README; it does not read it in a test), so a `.md`
file outside `docs/` is still docs. `cli` is the only host-only crate the
kernel never links; `xtask` builds the kernel and stays code. Pushes to
main and manual runs are classified code whatever they touch.

A third-party action (`dorny/paths-filter`) would do the same in YAML, but
it is one more dependency with access to the token, and its globs are not
testable on a laptop; the script is 30 lines with a test of its own that
the `docs` job runs.

Dry run on the last 30 closed pull requests (#86–#115): 4 docs (#90,
#106, #112, #115: designs, an archive, an audit scope), 26 code. The
README rewrite (#114) is code because it also changed `xtask/src/bench.rs`;
the archive batches (#92, #100) because they changed `bench/baseline.json`.
None was host-tools alone: `cli` changes so far came with kernel or xtask
changes.

## D2. What each class runs

| Job | docs | host-tools | code | main |
|---|---|---|---|---|
| `changes` | ✓ | ✓ | ✓ | ✓ |
| `docs`: specs, changed changes, Markdown links, classifier test, images when changed | ✓ | ✓ | ✓ | ✓ |
| `quick`: `cargo xtask check` with actionlint | | ✓ | ✓ | ✓ |
| `policy`: cargo-deny, cargo-machete | | ✓ | ✓ | ✓ |
| `control-plane`: `xtask --arch x86_64 test --only control` | | ✓ | (in `ci`) | (in `ci`) |
| `ci`: `cargo xtask --arch x86_64 ci` | | | ✓ | ✓ |
| `vmm`: boots and in-kernel tests on Firecracker and Cloud Hypervisor | | | ✓ | ✓ |
| `icount-x86_64`: `bench --icount-only --env ci --check` | | | ✓ | (in `bench.yml`) |
| `ci-arm64`: `cargo xtask --arch aarch64 ci` | | | ✓ | ✓ |
| `icount-aarch64`: `bench --icount-only --env ci-arm --check`, published | | | ✓ | ✓ |
| `miri` | | | ✓ | ✓ |
| `bench.yml`: every metric, timing gate | | | | ✓ and nightly |

The Markdown link check (`.github/scripts/check-links.py`) checks that
every relative link and image in a tracked `.md` file names an existing
path; it skips code blocks, inline code, anchors and URLs (an external
site's state is not the pull request's). `docs/img/check.js` needs
Chromium through Playwright (about a minute to install), so it runs only
when an `.svg` or a file under `docs/img/` changes.

## D3. `ci-ok`, the one required check

`ci-ok` needs every other job, runs `if: always()` and fails unless each
result is `success` or `skipped`, and the class is known. A job skipped
because a job it needs failed is harmless: the failed job is in the same
list. A cancelled job fails it. Branch protection would require `ci-ok`
only, so classes can add or drop jobs without touching the settings. The
repository has no branch protection and no rulesets today
(`gh api repos/davidsalvat/rhizo/branches/main/protection` answers 404
"Branch not protected"); setting it is left to the owner.

## D4. Fail fast: the quick job first

`quick` runs `cargo xtask check` (fmt, clippy for the host crates and for
the cells and kernel of both architectures, host tests, crate dependency
rules, actionlint): the checks most often red, in about a minute and a
half with a warm cache. Every heavy job `needs` it, so a format or lint
error costs one runner for a minute and a half instead of six runners for
up to nine minutes, and its report is the first thing an agent sees.

The cost is on the critical path: the heavy jobs start about 1.5 minutes
later. The AArch64 tests remain the longest job (306 s of `xtask ci` on
emulated AArch64, a third of it building kernels), so a code pull request
takes `changes` + `quick` + `ci-arm64`. Building fewer kernels (selecting
test modes at boot instead of with cargo features, worked on separately in
`kernel/` and `xtask/src/lib.rs`) shortens exactly that job; splitting the
AArch64 VM steps over two runners would be the next step if it is still
the longest, and needs an `xtask` option to run a subset of steps, so it
is left to that work rather than done in YAML with a list of `--only`
steps that would silently miss new ones.

## D5. Instruction counts gate pull requests; timings gate main

On a pull request the blocking benchmark gate is the `-icount` metrics, on
both architectures: exact counts of retired instructions under QEMU TCG,
the same on every host (spec `profiling`), at +2 %. `bench --icount-only`
already runs each count once (`profile::icount`: one run of the plain and
one of the profiled `bench-cell` kernel), so there are no medians to drop.
`--check` compares only metrics of the VMMs that ran (`bench::missing`),
so with `--env ci` only the 25 `qemu-microvm-icount` entries are compared
and the timing entries are neither measured nor reported missing: no
change to `xtask` is needed.

Timing benchmarks run in full with the `ci` gate in `.github/workflows/
bench.yml`, on every push to main, nightly (02:41 UTC) and by hand. So do
the metrics that have no instruction count: density and wake-up
percentiles, network round trips, Firecracker and Cloud Hypervisor wall
times, the comparisons with Linux, CRIU and gVisor, and the deterministic
sizes. Their thresholds and baseline are unchanged.

When that gate fails on main, a second job of the workflow, the only one
with `issues: write` (and `actions: read`; the workflow's default is
`contents: read`), opens the issue "bench: timing gate failing on main" or
comments on it when it is open: the failing metrics (from the `benchmark
regression:` and `benchmark not measured:` lines of the gate), the run,
and the commits since the head of the last successful `bench` run on main
(`gh api .../compare/<good>...<sha>`). It uses the workflow's
`GITHUB_TOKEN` with `gh api`, and runs no repository code.

The README data flow keeps working: `bench-results-N` annotations come
from the `bench` job of `bench.yml` on main, `bench-aarch64-results-N`
from `icount-aarch64`, which runs on main too.

## D6. Caching

A composite action (`.github/actions/setup`) does what each job repeated:
the pinned toolchain, `Swatinem/rust-cache@v2` (MIT, already used: it
caches `~/.cargo` registry and git and the dependencies in `target/`,
cleaning workspace crates out), MicroPython and CPython sources, QEMU, KVM,
and the CPython build and downloaded tools caches.

- rust-cache keys on the job id (one feature set per job), the compiler
  version from `rust-toolchain.toml`, every `Cargo.lock` and `Cargo.toml`,
  and `CARGO_*`/`RUST*` variables. It saves only on main
  (`save-if: github.ref == 'refs/heads/main'`): pull requests restore
  main's cache and cannot evict it with their own.
- It cannot poison results: workspace crates, the only artifacts that
  depend on the pull request's sources, are rebuilt every time; what is
  cached is registry crates, keyed on the lockfile.
- The CPython build is keyed on the hash of its recipe
  (`xtask/src/cpython.rs`, `xtask/src/packages.rs`) and checks the
  tarball's SHA-256; downloaded tools (Node.js, VMMs, reference guest) on
  `xtask/src/tools.rs`, which pins their checksums.

## D7. Flaky jobs: recorded, never retried

`.github/workflows/flaky.yml` runs on `workflow_run` of CI and bench. When
a run succeeded with `run_attempt > 1`, each job that failed in an earlier
attempt failed on the same commit: it is flaky. For each, it opens an
issue titled `flaky: <first FAILED line of its log>` (for example `flaky:
bench: benchmark regression: cell.fork_with_files_us/rhizo/qemu-microvm`),
or the job and step when the log has none, or comments on the open issue
with that title, with links to the failed job and the passing attempt.
Nothing is retried automatically and no test is skipped (rule 9); the
issue is the cue to fix the cause. A dry run of its queries on run
44916302134 produced the title above. It runs from the default branch's
copy, so it starts working once merged.

## D8. Working with many PRs

AGENTS.md gets the rules the coordinator and agents follow when many pull
requests are open at once: one open pull request per hotspot
(`kernel/src/cell.rs` core, `kernel/src/tests.rs`, the snapshot format
version, the control protocol version, `bench/baseline.json`,
`docs/security.md`), small stacked pull requests, merging main only on a
conflict GitHub reports, the coordinator merging in order and re-running
CI only for pull requests in the same area, version numbers reserved
before work starts, and semantic conflicts fixed by a small patch.

## Result

Measured on this pull request's own CI runs; see `tasks.md` 4.1 and the
pull request description.
