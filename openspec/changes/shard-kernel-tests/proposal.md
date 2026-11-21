# Proposal

## Why

`faster-ci-2` left the in-kernel test shards (`RHIZO_SHARDS`) out of CI
because one test depended on its order: with `RHIZO_SHARDS=2` or `4` on
`ktest`, and `2` on `ktest-smp`, on AArch64,
`dynamically_linked_programs_behave_as_on_linux` (now in
`kernel/src/tests/dynamic.rs`) found 2 heap allocations fewer after it
than before it. The AArch64 test parts in CI end with the in-kernel tests
of one VM (about 98 s on one CPU, 82 s on four), while the host tests
beside them take 65 s.

## What Changes

- **Root cause fixed.** A connection through the network card that an
  earlier test closed stays in `smoltcp` until the peer acknowledges the
  close, and holds two heap allocations until then. The leak checks'
  shared helper, `memory_balance()`, now first waits (at most 10 s) until
  no connection through the card is still closing (`settle_wire()`, the
  loop two tests already had), as it already drops the kept images. A
  connection a test leaves open still fails it.
- **Shards in CI.** The AArch64 parts of `ci-arm64` run their in-kernel
  tests split over two VMs each (`RHIZO_SHARDS=2`).
- **A shard's failure reproduces as printed.** The `reproduce` command of a
  split step carries `RHIZO_SHARDS=N`.
- `docs/security.md` points to `kernel/src/tests/isolation.rs` for the H13
  tests.

## Impact

- Specs: `kernel-testing` (tests do not depend on their order: card
  connections settle before a memory check), `developer-feedback` (CI
  splits the AArch64 in-kernel tests; a split step's reproduce command).
- Code: `kernel/src/tests/density.rs`, `cpython.rs`, `packages.rs`;
  `xtask/src/lib.rs`; `.github/workflows/ci.yml`; `docs/security.md`.
- Coverage: every in-kernel test still runs on every code pull request,
  exactly once per step.
