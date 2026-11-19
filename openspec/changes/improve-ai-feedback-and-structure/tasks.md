# Tasks

## 1. Feedback loop

- [x] 1.1 Add the failure report type (kind, what, where, expected, actual, hint, reproduce) with human and JSON renderings and route every `xtask` failure through it; verify unit tests for both renderings
- [x] 1.2 Parse `file:line` and messages from kernel panic and exception reports into failure reports; verify with a unit test on captured output
- [x] 1.3 Add `--json` to `check`, `test`, `ci` and `bench` (one object per line); verify a unit test parses the emitted lines
- [x] 1.4 Add `cargo xtask check` (fmt, clippy for host crates and kernel, host tests, dependency rules) with its duration; verify it runs without QEMU and record its warm duration
- [x] 1.5 Add `cargo xtask test --only <name>`; verify `--only ktest-page-fault` runs just that self-check
- [x] 1.6 Fix the proptest seed in `.cargo/config.toml` `[env]`; verify two runs generate the same cases and an overriding `PROPTEST_RNG_SEED` is respected

## 2. Structure

- [x] 2.1 Move `rhizo-core::memory` into a new `rhizo-mm` crate and update users; verify all tests pass and deterministic benchmark metrics are unchanged
- [x] 2.2 Encode the crate dependency table and check it in `xtask check`; verify a forbidden edge is reported with both crates and the rule
- [x] 2.3 Document the code map and dependency table in AGENTS.md; verify it matches `cargo metadata`

## 3. Quality gates

- [x] 3.1 Add the workspace lints (dbg_macro, todo, unimplemented, print_stdout/print_stderr outside xtask, redundant_clone, needless_pass_by_value, unused_self) and fix findings; verify `cargo xtask check` passes
- [x] 3.2 Add `deny.toml` (no GPL/AGPL/LGPL, advisories) and `cargo-machete`, both in CI; verify they pass in CI (the license list is an allow-list, so any GPL/AGPL/LGPL license is rejected by construction)
- [ ] 3.3 Run the heap tests under Miri in CI; verify the job passes
- [ ] 3.4 Add `cargo xtask mutants` and a weekly workflow (plus random-seed proptest run); verify locally on `rhizo-mm` and record surviving mutants

## 4. Agent rules and tooling

- [x] 4.1 AGENTS.md rules: behavior-level tests, no weakening/deleting tests without justification, minimal code, justified dependencies, delete dead code; verify `cargo xtask ci` passes
- [x] 4.2 Version project skills under `.agents/skills/` in the Agent Skills format (OpenSpec change, in-kernel test, benchmarks, debugging a boot); update `.gitignore`; verify each SKILL.md has valid frontmatter
- [x] 4.3 CONTRIBUTING.md: optional local tools (nextest, cargo-bloat, cargo-show-asm) and the criteria for third-party skills (none qualifies yet, so none is listed); verify links

## Workflow follow-up

- Archive this change with `openspec archive improve-ai-feedback-and-structure` after it is merged.
