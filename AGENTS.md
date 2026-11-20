# AGENTS.md

Instructions for AI coding agents working on Rhizo. This file follows the open
[AGENTS.md](https://agents.md) convention and is meant to work with any agent or
model. Human contributors should read [CONTRIBUTING.md](CONTRIBUTING.md) too.

## Project

Rhizo is an MIT-licensed operating system kernel, written in Rust, whose basic
unit is the **agent cell**: an isolated environment that can be created,
suspended, resumed and forked natively. See [README.md](README.md) for the
thesis and roadmap.

The project is a proof of concept. AArch64 is the primary architecture and
x86_64 the second one; both run under QEMU, Firecracker and Cloud Hypervisor,
and every spec must hold on both.

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
- The archive pull request of a change that completes a milestone or adds a
  headline metric also brings the README up to date: take one benchmark run
  (`bench-results.json`, from CI's `bench-results-N` annotations), run
  `python3 docs/img/gen.py <it>` for the graphics and
  `cargo xtask bench --from <it> --readme` for the tables, and check every
  changed image in light and dark themes (no clipped or overlapping text).

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
7. **Speed first.** Among correct options, choose the fastest. Every
   optimization ships with a benchmark that demonstrates it (a metric in
   `cargo xtask bench`, see `bench/README.md`).
8. **No silent regressions.** No pull request may make a benchmark metric
   worse without a written justification in the pull request; updating
   `bench/baseline.json` is an explicit, justified change.
9. **Tests check behavior, and tests are not weakened.** Tests assert
   observable behavior through public interfaces, not internal details.
   Never weaken, skip, `#[ignore]` or delete a test, or loosen an assertion,
   to make it pass. If the expected behavior really changed, change the spec
   first and justify the test change in the pull request.
10. **Minimal code.** Write the smallest solution that meets the spec: no
    speculative abstractions, no generality nobody uses yet, no new
    dependency without a justification in the design (license included).
    Delete dead code instead of commenting it out.

## Conventions

- Language for code, comments, specs and commits: **English**.
- Rust edition 2024, stable toolchain pinned in `rust-toolchain.toml`.
  Format with `rustfmt`; code must pass `clippy` without warnings.
- Commit messages: imperative mood, short summary line, explain *why* in the
  body. Reference the OpenSpec change id when relevant.
- Keep pull requests scoped to a single change.

## Build and test

Requires the Rust toolchain (via rustup), `qemu-system-aarch64` and, for the
second architecture, `qemu-system-x86_64`, plus gcc with a static glibc for
the C cells (`gcc`, `libc6-dev`; to cross-build, `gcc-aarch64-linux-gnu` or
`gcc-x86-64-linux-gnu`). Every `cargo xtask` command takes
`--arch aarch64` (default) or `--arch x86_64` right after `xtask`.

The feedback loop, fastest first:

| Command | Time | What |
|---|---|---|
| `cargo test -p <crate> <name>` | < 1 s | One host test |
| `cargo xtask check` | seconds | fmt, clippy (all crates), host tests, crate dependency rules. **Run after every edit.** |
| `cargo xtask test --only <step>` | seconds | One VM step: `boot`, `ktest`, or a self-check such as `ktest-page-fault` |
| `cargo xtask test` | ~1 min | Host tests, boot check, in-kernel tests, all self-checks |
| `cargo xtask ci` | minutes | Everything CI runs. **Run before pushing.** |
| `cargo xtask bench` | minutes | Release boot benchmarks per VMM (`--env ci --check` gates) |
| `cargo xtask mutants` | long | Mutation testing (weekly in CI) |

- Add `--json` to `check`, `test`, `ci` or `bench` for one JSON object per
  line on stdout (`step`, `failure`, `data`, `summary`).
- Every failure prints `FAILED <kind>` with `where`, `expected`, `actual`,
  `hint` and an exact `reproduce` command. Start from `reproduce`.
- Property tests use a fixed seed (`.cargo/config.toml`); override with
  `PROPTEST_RNG_SEED`. Commit any `proptest-regressions/` file a failure
  creates.
- VMMs: AArch64 `qemu-virt` (default), x86_64 `qemu-microvm` (default) and
  `qemu-q35`; both `firecracker` and `cloud-hypervisor` (need KVM for the
  kernel's architecture). The kernel reads `console=` and `exit=` from its
  command line (PVH on x86_64, `/chosen/bootargs` on AArch64); `xtask` passes
  the right values.
- Project skills (open Agent Skills format) live in `.agents/skills/`:
  `openspec-change`, `kernel-test`, `benchmarks`, `boot-debugging`.

## Code map

| Crate | Owns | May depend on (workspace) |
|---|---|---|
| `crates/rhizo-hal` | Hardware interfaces (traits) and host test doubles | — |
| `crates/rhizo-core` | Boot reports, boot options, device-tree reader, exception reports (both architectures), in-kernel test runner | `rhizo-hal` |
| `crates/rhizo-mm` | PVH memory map, frame allocator, page tables (`EntryFormat`: `X86_64`, `Aarch64`), copy-on-write that shares page tables (`cow`: `Frames` with reference counts, fork, write resolution), kernel heap | `rhizo-hal` |
| `crates/rhizo-cell` | Cells as plain data, static ELF loader and initial stack, user-memory access, cell address spaces (anonymous mappings, teardown) | `rhizo-hal`, `rhizo-mm` |
| `crates/rhizo-fs` | A cell's in-memory file system: tree, file blocks shared copy-on-write between forks, descriptors, pipes, event counters, `epoll` instances, socket descriptors and Unix socket names, encoding for snapshots | — |
| `crates/rhizo-net` | The kernel's sockets, shared by all cells: loopback TCP and UDP, Unix stream sockets, readiness, options, change counter for waiting cells, fork and snapshot records | — |
| `crates/rhizo-abi` | Linux system calls of cells (both conventions), files through the cell's `rhizo-fs`, pipes, `poll`/`select`/`epoll`, sockets through `rhizo-net`, `EFAULT`/`ENOSYS`, scheduling outcomes (yield, sleep, block, exit) | `rhizo-hal`, `rhizo-mm`, `rhizo-cell`, `rhizo-fs`, `rhizo-net` |
| `crates/rhizo-snapshot` | Snapshot format of a suspended cell (versioned, little endian, files and sockets sections), capture from and restore into a page table | `rhizo-hal`, `rhizo-mm`, `rhizo-cell`, `rhizo-fs`, `rhizo-net` |
| `crates/rhizo-sched` | Run queue, sleep list, waiting set and time slice (time passed in, never read); address-space tag pool | — |
| `kernel` | Platform code (`src/arch/aarch64`, `src/arch/x86_64`) and wiring; the cell host, scheduler loop and the kernel's sockets (`src/cell.rs`); shared in-kernel tests (`src/tests.rs`) | `rhizo-hal`, `rhizo-core`, `rhizo-mm`, `rhizo-cell`, `rhizo-abi`, `rhizo-sched`, `rhizo-snapshot`, `rhizo-fs`, `rhizo-net` |
| `cells` | Test programs run as cells: `no_std` Rust static executables (`src/bin`) and real C programs linked statically with the system glibc (`c/`, built by `xtask` with the native gcc or `aarch64-linux-gnu-gcc`/`x86_64-linux-gnu-gcc` when cross-building); expectations in `cells/expected.rs`, also checked natively on a Linux host | — |
| `xtask` | Developer tooling (host only) | — |

Planned crates are created by the milestone that adds their code, not
before.

Rules, checked by `cargo xtask check` (`xtask/src/deps.rs`): no cycles;
`rhizo-hal` depends on nothing; only `kernel` contains platform code
(assembly, ports, control registers); library crates never depend on
`kernel`. A new crate adds its row to `xtask/src/deps.rs` and to this table in
the same pull request. Prefer putting logic in a library crate so it is
tested on the host.

The `arch` interface (`kernel/src/arch/mod.rs` lists it) is the only thing
shared kernel code may use: `boot`, `platform`, `console`, `power`, `cpu`,
`timer`, `tests`, and, compiled only in builds that run cells, `user`
(entering and leaving cell code, address-space tags) and `irq` (the
preemption timer; interrupts arrive only while cell code runs). Anything instruction-set specific (assembly, system
registers, ports, MMIO registers, entry encodings) goes behind it. New arch
hooks are written for AArch64 first and must reach x86_64 in the same
milestone. Tests that need architecture details go in `arch/<cpu>/tests.rs`.

Memory layout, the same on both architectures (see
`openspec/specs/virtual-memory`):

- Lower half (`0x0000_…`): the running cell (image at its link address,
  anonymous mappings from `0x0000_4000_0000_0000`, stack below
  `0x0000_7fff_ffff_f000`); empty while no cell runs.
- `0xffff_8000_0000_0000 + phys`: direct map of RAM (RW, NX). Access physical
  memory only through `PhysicalMemory` (kernel: `memory::DirectMap`).
- `0xffff_ffff_8000_0000 + phys`: kernel image with W^X; the page under the
  kernel stack is an unmapped guard page.
- AArch64 only: `0xffff_c000_0000_0000 + phys` is a device-memory window
  for MMIO (the UART); the kernel half lives in `TTBR1_EL1`, and `TTBR0_EL1`
  (cells) is an empty table.
- Kernel globals use `sync::Global` (single CPU, panics on reentrance).

The kernel crate only builds for `aarch64-unknown-none-softfloat` and
`x86_64-unknown-none`; plain `cargo` commands skip it (it is not a default
workspace member). Use `cargo xtask` for it.

## Tool-specific files

Do not commit configuration that only one AI tool understands (for example
`CLAUDE.md`, `.claude/`, `.cursor/`, `.gemini/`, generated OpenSpec slash
commands). They are listed in `.gitignore`. This file is the single shared
source of instructions.
