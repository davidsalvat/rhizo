# Proposal

## Why

Practically all of Rhizo's code will be written by AI coding agents, and the
place where agents get stuck is the feedback loop: slow checks, failures that
do not say what to do, output only a human can parse, failures that cannot be
reproduced, and tests that are easier to "fix" than the code. The structure of
the repository also matters: an agent with a limited context must find where
things go and must not be able to cross subsystem boundaries by accident.

This change sits after `add-fast-boot-and-benchmarks` and before H2, so every
later milestone is built with this loop.

## What Changes

- **Fast loop:** `cargo xtask check` runs formatting, lints and host tests for
  everything in seconds, without QEMU.
- **Actionable failures:** every failure reported by `xtask` (host test,
  in-kernel test, self-check, benchmark, lint, dependency rule) states what
  failed, where (`file:line` when known), expected versus actual, and the exact
  command that reproduces it.
- **Machine-readable output:** `--json` on `check`, `test`, `ci` and `bench`
  emits structured results (one JSON object per event) next to the human
  output.
- **Reproducibility:** property tests run with a fixed, printed seed; failing
  cases are persisted and committed; every reported failure carries a
  reproduce command.
- **Tests that cannot be gamed:** AGENTS.md rules: tests verify observable
  behavior; weakening, skipping or deleting a test to make it pass requires a
  written justification. Mutation testing (`cargo-mutants`) on the
  hardware-independent crates, run weekly and on demand, reports tests that do
  not test anything.
- **Minimal code:** AGENTS.md rule (smallest solution that meets the spec, no
  speculative abstractions, no unjustified dependencies, delete dead code),
  enforced where tools can: stricter clippy lints, `cargo-deny` for licenses
  and advisories, an unused-dependency check.
- **Structure for growth:** split memory management out of `rhizo-core` into
  `rhizo-mm`, define the target crate map up to H7 with dependency rules
  checked by `xtask`, and document the code map in AGENTS.md. New crates are
  created when their milestone arrives, not before.
- **Agent tooling at project level:** version project skills in the open Agent
  Skills format under `.agents/skills/`; keep tool-specific files local and
  ignored; document optional local installs in CONTRIBUTING.md. Add `miri` for
  the `unsafe` heap code in CI.

### Out of scope

Kernel features. Coverage gating. Third-party skills of unclear origin.

## Capabilities

### New Capabilities

- `developer-feedback`: the fast check command, failure reports, JSON output
  and reproducibility guarantees of the developer tooling.
- `code-quality-gates`: lints, dependency and license policy, mutation and
  `unsafe` checks, and the crate dependency rules.

### Modified Capabilities

None.

## Impact

- `xtask`: `check` command, `--json`, failure report type, dependency-rule
  checker, `deny`/`mutants`/`miri` integration.
- Workspace: new `crates/rhizo-mm` (moved from `rhizo-core::memory`).
- New files: `deny.toml`, `.agents/skills/*`, `proptest-regressions/` when
  failures occur.
- `.gitignore`: stop ignoring `.agents/` (now the standard skills location).
- CI: `cargo-deny` and `miri` jobs; weekly mutation-testing workflow.
