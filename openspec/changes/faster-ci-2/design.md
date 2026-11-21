# Design

## Baseline (before the change)

The code pull-request run of `fewer-kernel-builds` (44932285327, #122),
from `gh api repos/davidsalvat/rhizo/actions/runs/<id>/jobs` and the job
logs. Start is minutes after the run's first job started.

| Job | Start | Duration |
|---|---|---|
| `changes` | 0.0 | 0.1 min |
| `quick` (`xtask check`) | 0.2 | 1.5 min |
| `policy` | 0.2 | 0.8 min |
| `ci-arm64` (AArch64 tests) | 1.8 | 4.7 min |
| `icount-aarch64` | 1.8 | 3.3 min |
| `ci` (x86_64 tests) | 1.8 | 3.3 min |
| `vmm` (Firecracker, Cloud Hypervisor) | 1.8 | 2.4 min |
| `icount-x86_64` | 1.8 | 1.7 min |
| `miri` | 1.8 | 3.7 min |
| **Wall clock** | | **6.65 min** |

The critical path is `changes`, `quick`, then `ci-arm64`. Inside
`ci-arm64` (4-CPU ARM64 runner, no KVM): setup 34 s; building `xtask`
and the lints of 7 kernel feature sets 20 s; the differential check 14 s;
building the host tests and the kernels 42 s; then the VM steps at once
for 170 s: host tests 66.8 s, in-kernel tests 92.4 s, on 4 CPUs 83.8 s,
control plane 34.6 s, Node.js 25.9 s, PyPI packages 19.0 s, the hang
self-check 5.2 s, the other seven self-checks and the boot check 0.1 to
0.2 s each. Weighted by the host CPUs each keeps busy (`Job::weight`),
that is about 560 CPU-seconds on 4 CPUs: the VM phase is CPU-bound.

Miri: 3.7 s of build, then 186 s of tests run one after another by Miri:
`allocations_never_overlap` 85.3 s, `whole_free_pages_go_back_when_due`
82.5 s, `sort_orders_a_free_list` 14.6 s, the ten others 4 s together.

## D1. Every job starts after `changes`, and a failing `quick` cancels the run

The heavy jobs `need` only `changes`, so they start 1.5 minutes earlier.
`quick` gets `actions: write` (the only job that does) and, when it
fails, posts `POST /repos/{repo}/actions/runs/{id}/cancel`: the jobs
still running stop within seconds. `ci-ok` runs `if: always()`, so it
still runs and fails on `quick`'s failure.

Cost. Since `quick` gated the heavy jobs (16:30 on 2026-11-21), 2 of 12
pull-request runs failed in `quick` (44925949516, 44925716151). Without
the cancellation, each would have used seven more runners for up to 4.7
minutes; with it, for about the 1.5 minutes `quick` takes: about 10
runner-minutes per failing run, 1.7 per pull request on average, on a
public repository whose standard and ARM64 runners are free. Against
that, every code pull request saves 1.5 minutes of wall clock. A pull
request from a fork gets a read-only token: its cancellation is refused,
and the other jobs finish (cost only; the result is the same).

## D2. The AArch64 tests on two runners (`--part K/N`)

`cargo xtask ci --part K/N` and `test --part K/N` run one share of the
test steps. `xtask/src/parts.rs` deals them, most expensive first, each to
the part with the least cost so far (the longest-processing-time rule);
the costs are the CPU-seconds above, and a step without one costs 1. The
deal depends only on the full step list and `N`, so the parts of one `N`
hold every step exactly once, a new step included; a test checks it for
1 to 3 parts. `ci` lints in part 1 only, and runs its x86_64 q35 steps
there. For two parts:

| Part | Steps | Cost |
|---|---|---|
| 1 | `ktest-smp`, `control`, `ktest-packages`, `differential`, `ktest-user-read`, and the lints | 290 |
| 2 | `host`, `ktest`, `ktest-node`, `ktest-hang`, `boot`, six self-checks | 290 |

Part 2 is bounded by the in-kernel tests on one CPU (92 s), part 1 by
those on 4 CPUs (84 s) plus the lints and the differential check. A
list of `--only` steps in the workflow was the alternative; it would
silently miss a new step. Both parts keep the job's `rust-cache` key
(`ci-arm64`), so they restore main's dependencies.

**In-kernel test shards are not used in CI.** Splitting the one-CPU
tests too (`RHIZO_SHARDS`) would bring part 2 under a minute of VM time,
but the tests are not all independent of their order. On AArch64 (this
host, 4 CPUs, no KVM):

| Run | Result |
|---|---|
| `RHIZO_SHARDS=2 test --only ktest` | part 1 ok (23.4 s); part 2 fails |
| `RHIZO_SHARDS=3 test --only ktest` | ok: 23.2, 37.8, 24.5 s |
| `RHIZO_SHARDS=4 test --only ktest` | parts 1, 3, 4 ok (17.5, 12.9, 42.3 s); part 2 fails |
| `RHIZO_SHARDS=2 test --only ktest-smp` | part 1 ok (35.1 s); part 2 fails |
| `test --only ktest` (no shards) | ok, 75.2 s |

Every failure is the same: `dynamically_linked_programs_behave_as_on_linux`
(`kernel/src/tests.rs`, "nothing leaked" at line 2740), whose heap
allocation count after the test is 2 lower than before (1038 against
1040, 1021 against 1023): two allocations made before it by a test that
runs earlier in the full order are freed by `drop_images` in it. It is
left to a follow-up (the kernel tests are being split into modules by
another change); once fixed, `RHIZO_SHARDS=2` on part 2 is the next step.

## D3. Miri in three processes

Miri interprets one test at a time, so the job builds once (`--no-run`)
and then runs three `cargo miri test` at once on the runner's 4 CPUs:
`--exact heap::tests::allocations_never_overlap`, `--exact
heap::tests::whole_free_pages_go_back_when_due`, and `heap::` skipping
those two. A new heap test lands in the third; a renamed slow test runs
there too, and its `--exact` process then runs nothing. The bound becomes
the slowest test, about 85 s, instead of 186 s. Every heap test still runs
on every code pull request, so no path filter on Miri is needed.

## D4. `test --affected` (local loop only)

`xtask/src/affected.rs` lists the paths changed since the merge base with
`origin/main` (`git diff --name-only <base>` plus untracked files), maps
each to its crate by directory (`crates/<name>`, `kernel`, `cells`,
`cli`), and adds every crate that depends on it through `deps::ALLOWED`.
Documentation paths (the classifier's docs class) reach nothing; a path of
`xtask` itself or of no crate (Cargo files, the toolchain, `.github`)
selects everything. Host tests run with `-p` for each reached crate but
the kernel and the cells; the kernel's VM steps run when the kernel or
the cells are reached, the differential check when the cells are, the
control plane when the kernel or `rhizo-cli` is. As every library crate
but `rhizo-cells` reaches the kernel, it mostly saves the host tests of
unrelated crates and the differential check, and for `cli/` changes, every
VM step but the control plane. CI does not use it: coverage there stays
complete.

## Rejected

- **Kernels built once and passed as artifacts.** The AArch64 job spends
  42 s building the host tests and the kernels, the instruction-count job
  about 25 s of release builds. A build job before the test jobs puts its
  setup (31 to 57 s) and its builds on the critical path, then an upload
  and a download (the Node.js and packages kernels embed 100 and 65 MiB),
  to save at most those 42 s per job: the critical path gets longer.
- **Caching workspace crates.** `rust-cache` restores dependencies only;
  cargo fingerprints workspace crates by file time, and a fresh checkout
  gives every file a new one, so cached workspace builds would be rebuilt
  anyway (CI builds with `CARGO_INCREMENTAL=0`). The warm/cold difference
  is in the dependencies, which are already cached: see *Result*.
- **A path filter on Miri.** With D3 Miri is off the critical path, and
  filtering would drop coverage of the heap code reached through
  `rhizo-hal` changes.

## Result

To be measured on this pull request's runs.
