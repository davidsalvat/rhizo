# AGENTS.md

Instructions for AI coding agents working on Rhizo. This file follows the open
[AGENTS.md](https://agents.md) convention and is meant to work with any agent or
model. Human contributors should read [CONTRIBUTING.md](CONTRIBUTING.md) too.

## Project

Rhizo is an MIT-licensed operating system kernel, written in Rust, whose basic
unit is the **agent cell**: an isolated environment that can be created,
suspended, resumed and forked natively. See [README.md](README.md) for the
thesis and roadmap.

The project is a proof of concept targeting x86_64 under QEMU/KVM.

## Workflow: specs first (OpenSpec)

This repository uses spec-driven development with
[OpenSpec](https://github.com/Fission-AI/OpenSpec).

- `openspec/specs/` holds the **current** behavior of the system. It is the
  source of truth.
- `openspec/changes/<change-id>/` holds proposed changes: `proposal.md`,
  `design.md`, `tasks.md` and delta specs.
- Do not implement behavior that is not described in an approved change.
  If you need new behavior, propose a change first.
- When implementing a change, follow its `tasks.md` in order and tick each
  task (`- [x]`) only after its stated verification passes.
- Validate changes with `openspec validate <change-id> --strict`.
- After a change is merged, archive it with `openspec archive <change-id>`
  so its deltas are folded into `openspec/specs/`.

## Non-negotiable rules

1. **No GPL or AGPL code.** Never copy code from Linux, LTP, or any copyleft
   project. Reading for understanding is fine; copying is not. Permissive
   code (MIT, BSD, Apache-2.0) may be reused only with its license notice kept.
2. **Hardware boundary.** Kernel core logic must not touch hardware directly.
   All hardware access (console, clock, interrupts, memory-mapped devices,
   power) goes through narrow interfaces so the core can run in a
   deterministic simulator on the host. No hidden dependencies on wall-clock
   time or ambient randomness in core logic.
3. **Every change ships its tests.** Logic that can run on the host gets host
   unit or property tests; hardware-facing code gets in-QEMU tests.
4. **Performance is part of correctness.** Do not regress benchmarked paths.
5. **`unsafe` must be justified.** Every `unsafe` block carries a `// SAFETY:`
   comment explaining why it is sound. Keep `unsafe` in the smallest possible
   scope, ideally inside hardware-facing modules.
6. **Cell state must be serializable.** Any per-cell kernel state (memory,
   registers, file descriptors, timers…) must be designed so it can be
   snapshotted and cloned. This is the core of the project.

## Conventions

- Language for code, comments, specs and commits: **English**.
- Rust edition 2024, stable toolchain pinned in `rust-toolchain.toml`.
  Format with `rustfmt`; code must pass `clippy` without warnings.
- Commit messages: imperative mood, short summary line, explain *why* in the
  body. Reference the OpenSpec change id when relevant.
- Keep pull requests scoped to a single change.

## Build and test

Requires the Rust toolchain (via rustup) and `qemu-system-x86_64`.

- `cargo xtask build` / `cargo xtask run`: build / boot the kernel in QEMU.
- `cargo test`: host tests only (no QEMU).
- `cargo xtask test`: host tests, boot check, in-kernel tests in QEMU and
  harness self-checks.
- `cargo xtask ci`: formatting, clippy (host crates and every kernel feature
  set) and all tests. **Run this before proposing any change.**

Layout and where code belongs:

- `crates/rhizo-hal/`: hardware interfaces (traits) and host test doubles.
- `crates/rhizo-core/`: hardware-independent logic. Prefer putting logic here
  so it can be unit-tested on the host.
- `kernel/src/arch/`: the only place for assembly, port I/O and other direct
  hardware access.
- `kernel/src/tests.rs`: in-kernel tests (run inside QEMU).
- `xtask/`: developer tooling.

The kernel crate only builds for `x86_64-unknown-none`; plain `cargo` commands
skip it (it is not a default workspace member). Use `cargo xtask` for it.

## Tool-specific files

Do not commit configuration that only one AI tool understands (for example
`CLAUDE.md`, `.claude/`, `.cursor/`, `.gemini/`, generated OpenSpec slash
commands). They are listed in `.gitignore`. This file is the single shared
source of instructions.
