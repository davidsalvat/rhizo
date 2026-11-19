# Design

## Context

See `proposal.md`. Today: `cargo xtask ci` takes minutes (QEMU boots and seven
kernel builds); `cargo test` is fast but covers only host crates; failures are
human-readable text; proptest uses random seeds; `rhizo-core` mixes boot
reporting, diagnostics and all memory management; `.agents/` is ignored.

## Goals / Non-Goals

**Goals:** feedback in seconds for most edits; every failure tells an agent
what to do next; structure that keeps an agent's working set small.

**Non-Goals:** coverage targets; replacing cargo's own output formats.

## Decisions

### D1. `cargo xtask check` (the fast loop)

Runs, in order, stopping at the first failure: `cargo fmt --check`,
`cargo clippy` on host crates and on the kernel (default features only),
host tests, the crate dependency rules. No QEMU, no release builds. Target:
under 30 s warm on a laptop; its duration is printed so regressions are
visible. `ci` stays the complete gate.

### D2. One failure type, two renderings

`xtask` reports every failure through one structure:

```text
FAILED <kind>: <what>
  where:     <file:line or command>
  expected:  <...>
  actual:    <...>
  hint:      <what usually fixes it>
  reproduce: <exact command>
```

With `--json`, each check emits `{"event": "step"|"failure"|"summary", ...}`
lines with the same fields. In-kernel failures already carry `file:line` in
the panic report; `xtask` parses it into `where`. Reproduce commands are exact
(`cargo xtask test --only <self-check>`, `cargo test -p <crate> <test>`,
`PROPTEST_RNG_SEED=<seed> cargo test ...`).

### D3. Reproducible property tests

`.cargo/config.toml` sets `PROPTEST_RNG_SEED` in its `[env]` table, so even a
plain `cargo test` explores the same cases on every machine; an explicit
environment variable overrides it. Proptest's failure persistence stays on and
`proptest-regressions/` files are committed, so a found counterexample becomes
a permanent regression test. A weekly job runs with a random seed and more
cases (`PROPTEST_CASES`), and prints the seed of any failure.

### D4. Rules against gaming tests and against excess code

AGENTS.md gains: tests assert observable behavior through public interfaces;
weakening, skipping or deleting a test requires a justification in the pull
request; the smallest solution that meets the spec; no speculative
abstractions or unused generality; every new dependency justified in the
design; dead code deleted, not commented out.

Enforcement: workspace clippy lints `dbg_macro`, `todo`, `unimplemented`,
`print_stdout`/`print_stderr` (except in `xtask`), `undocumented_unsafe_blocks`
(already), `redundant_clone`, `needless_pass_by_value`, `unused_self`;
`cargo-deny` (licenses: deny GPL/AGPL/LGPL in dependencies; advisories;
duplicate versions as warnings); `cargo-machete` for unused dependencies.

### D5. Mutation testing and `unsafe` checking

- `cargo-mutants` on `rhizo-mm` and `rhizo-core`: too slow per pull request
  (it rebuilds per mutant), so it runs weekly and on demand
  (`cargo xtask mutants`), and its surviving mutants are reported as issues to
  fix by adding tests.
- `miri` runs the heap and allocator host tests (the code with raw pointers)
  in CI. It needs a nightly toolchain only for that job; the pinned stable
  toolchain stays for everything else.

### D6. Structure for growth — options

| Option | What | For | Against |
|---|---|---|---|
| A. Keep three crates, document only | Target map in AGENTS.md, migrate later | Zero churn now | `rhizo-core` keeps growing; boundaries enforced only by review |
| B. Split everything now | Crates for every future subsystem (`mm`, `cell`, `abi`, `sched`, `snapshot`) | Final layout from day one | Empty crates are speculative structure, which the minimal-code rule forbids |
| **C. Split what exists, grow by milestone (recommended)** | Move memory management to `rhizo-mm` now; each later milestone creates its crate when it adds the code; `xtask` checks dependency rules | Small, focused crates where code exists; per-crate `cargo test -p` keeps the loop fast; rules enforced by a tool | One move now (mechanical, covered by tests) |

Target map (C):

| Crate | Owns | Arrives | May depend on |
|---|---|---|---|
| `rhizo-hal` | Hardware interfaces and host doubles | now | — |
| `rhizo-mm` | Memory map, frames, page tables, heap | now (moved) | `hal` |
| `rhizo-core` | Boot reports, options, diagnostics, in-kernel test runner | now | `hal` |
| `rhizo-cell` | Cell model, address spaces, serializable cell state | H2 | `hal`, `mm` |
| `rhizo-abi` | Linux syscall layer | H2 | `hal`, `mm`, `cell` |
| `rhizo-sched` | Scheduling, cell switching (PCID) | H3 | `hal`, `cell` |
| `rhizo-snapshot` | Suspend/resume, lazy restore, fork, copy-on-write, templates | H4–H5 | `hal`, `mm`, `cell` |
| `kernel` | x86_64 platform code and wiring only | now | all of the above |
| `xtask` | Developer tooling | now | — (host only) |

Rules: no cycles; only `kernel` contains platform code; library crates never
depend on `kernel`; `hal` depends on nothing. `xtask check` reads
`cargo metadata` and fails on any edge outside the table.

### D7. What is versioned

| Item | Options | Recommendation |
|---|---|---|
| Shared agent instructions | AGENTS.md (standard) | Versioned (already) |
| Project skills (how to add an in-kernel test, run benchmarks, make an OpenSpec change) | `.agents/skills/<name>/SKILL.md`, the Agent Skills open standard read by 30+ tools | **Versioned**: stop ignoring `.agents/`; only `skills/` inside it is committed |
| Third-party skills and plugins | Copy into the repo, or install locally | **Local only**, listed in CONTRIBUTING.md with license and maintainer; none is required |
| Tool-specific config (`CLAUDE.md`, `.cursor/`, slash commands…) | — | Ignored (unchanged) |

Third-party skills are only listed if they come from the tool vendor or a
well-known maintainer, carry an OSI license, and ask for no credentials. If
none qualifies for a topic, the project writes its own skill instead.

### D8. Rust tools evaluated

| Tool | Decision | Why |
|---|---|---|
| `cargo-deny` | Adopt in CI | Enforces the no-copyleft rule on dependencies automatically |
| `cargo-machete` | Adopt in CI | Cheap unused-dependency check (stable toolchain) |
| `miri` | Adopt in CI (heap tests) | The only `unsafe` pointer logic outside platform code |
| `cargo-mutants` | Weekly + on demand | Finds tests that test nothing; too slow per PR |
| `cargo-nextest` | Optional, used by `xtask` when installed | Faster and machine-readable, but the suite is small today; not worth a required install |
| `cargo-bloat`, `cargo-asm` | Documented, on demand | Useful for size/codegen work; `image.size_bytes` already gates size |
| Coverage (`cargo-llvm-cov`) | Not now | Mutation testing answers the real question (do tests detect bugs?) |

## Risks / Trade-offs

- [More CI jobs, longer CI] → `deny` and `machete` take seconds; `miri` runs
  only the heap tests; mutation testing is off the PR path.
- [Fixed seeds explore less] → a weekly job runs property tests with random
  seeds and many more cases; failures print the seed to pin.
- [Crate split churn] → a single mechanical move now, verified by the existing
  tests and benchmarks (deterministic metrics must not change).
