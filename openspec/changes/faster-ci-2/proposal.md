# Proposal

## Why

After `faster-ci` and `fewer-kernel-builds`, a code pull request still
takes about 6.5 to 8 minutes of wall clock (design, *Baseline*), and that
is the agents' feedback loop. The critical path is three jobs in a row:
classify (0.1 min), then the quick checks (1.5 min), which every heavy job
waits for, then the AArch64 tests (4.7 min), whose VM steps share one
4-CPU runner for 170 s. Miri (3.7 min) runs its 13 heap tests one after
another in one process, two of them for 85 s each.

## What Changes

- **Every job starts at once.** The heavy jobs need only `changes`, not
  `quick`. When `quick` fails it cancels the run through the REST API, so
  the others stop within seconds instead of running for minutes; `ci-ok`
  still fails on it.
- **The AArch64 tests on two runners.** `cargo xtask ci --part K/N` (and
  `test --part K/N`) runs one share of the test steps; the shares are dealt
  by each step's measured cost, so the parts of one `N` hold every step
  exactly once, a new step included. Part 1 also lints. `ci-arm64` becomes
  a two-part matrix.
- **Miri in three processes.** The two slowest heap tests run each in a
  Miri process of its own, beside a third with every other heap test.
- **`cargo xtask test --affected`, for the local loop.** From the files
  changed against `origin/main` and the crate dependency table, it picks
  the host test crates and the VM steps a change can reach. CI keeps
  running everything.

Considered and not done (design, *Rejected*): in-kernel test shards in CI
(one test depends on the order), a separate build job passing kernels as
artifacts, caching workspace crates, and a path filter on Miri.

## Impact

- Specs: `developer-feedback` (quick checks cancel the run instead of
  gating it; test steps split over machines; affected steps locally).
  `code-quality-gates` is unchanged: Miri still runs every heap test.
- Code: `.github/workflows/ci.yml`; `xtask/src/lib.rs`, new
  `xtask/src/parts.rs` and `xtask/src/affected.rs`; AGENTS.md (the
  feedback loop table).
- Coverage: every test and step that ran on a code pull request still runs
  on every code pull request.
- Cost: a pull request whose quick checks fail uses the other runners for
  as long as `quick` takes (about 1.5 minutes each) before they are
  cancelled; one more ARM64 runner per code pull request.
