# Rhizo

**An operating system kernel built for AI agents.**

Rhizo is an open-source (MIT) kernel whose basic unit is not the process or the
container, but the **agent cell**: an isolated execution environment that can be
created, suspended, resumed and forked as native kernel operations.

> **Status: pre-alpha proof of concept.** Nothing here is usable yet. Follow the
> roadmap below to see where we are.

## Why

Today an AI agent that runs code sits on top of a tall stack:

```
orchestrator → host Linux → VMM or userspace kernel → (often) guest Linux → agent
```

Every layer was designed for something else, and the features agents need most
(fast suspend/resume, cheap cloning of the full environment, strict isolation of
untrusted code, fine-grained permissions) are bolted on from the outside.

Agent workloads look nothing like classic services:

| Classic service | AI agent |
|---|---|
| Long-lived, always on | Short bursts, mostly idle (waiting for a model or a human) |
| Trusted code | Untrusted, model-generated code |
| Tens per node | Thousands per node, most of them asleep |
| Scaling means more replicas | What matters is suspending and resuming quickly |
| Static permissions | Per-task permissions: tools, network, secrets |

Rhizo explores what happens when those needs are the design center of the
kernel itself.

The name comes from the *rhizome*: an underground stem that branches into
independent shoots (fork), can regrow a whole plant from a fragment (snapshot)
and lies dormant until it sprouts again (suspend/resume).

## Thesis

The proof of concept must demonstrate one measurable claim:

> A kernel whose unit is the agent cell can create, suspend, resume and fork
> cells running real Linux programs orders of magnitude faster than today's
> microVM- and userspace-kernel-based sandboxes.

Initial targets (hypotheses to be validated, not promises):

- `fork` of a ~50 MB cell: **< 1 ms**
- `resume` from memory: **< 100 µs**
- Overhead per idle cell: **a few KB** plus its own pages

## Principles

- **Primitives in the kernel.** Snapshot, resume, fork and capabilities are
  native kernel operations, not features added on top.
- **Linux ABI, not Linux code.** Cells run unmodified Linux binaries through a
  Linux-compatible syscall layer. No GPL code is copied into this repository.
- **Deterministic by design.** All hardware access goes through narrow
  interfaces so the kernel core can run inside a deterministic simulator.
- **Performance is a test.** Benchmarks run in CI with thresholds; a regression
  fails the build.
- **Specs first.** Every behavior is specified before it is built
  (see [`openspec/`](openspec/)).
- **Open for real.** MIT licensed, no proprietary core, no "open core" bait.
- **Model-agnostic.** Contribute with any AI coding agent, or with none.

## Roadmap (proof of concept)

| Milestone | Goal | Status |
|---|---|---|
| H0 | Boot in QEMU, serial console, in-kernel test harness, hardware boundary | Done |
| H1 | Physical memory, paging, kernel heap | — |
| H2 | First cell: user mode, ELF loader, basic Linux syscalls, static binaries | — |
| H3 | Multiple cells, scheduler, isolation tests | — |
| H4 | Suspend / resume with snapshot-equivalence tests | — |
| H5 | `fork(n)` with copy-on-write | — |
| H6 | Real workload (MicroPython) with differential tests against Linux | — |
| H7 | Reproducible benchmarks against existing sandboxes | — |

Out of scope for the proof of concept: networking, multicore, real disks,
hardware other than virtio/QEMU, full CPython, SDKs.

## Getting started

Prerequisites:

- [rustup](https://rustup.rs). The pinned toolchain (`rust-toolchain.toml`) is
  installed automatically on first use.
- QEMU for x86_64 (`qemu-system-x86_64`), version 8.2 or later is verified by
  CI; any version with PVH boot support (4.0+) should work. On Debian/Ubuntu:

```bash
sudo apt-get install qemu-system-x86
```

Commands:

| Command | What it does |
|---|---|
| `cargo xtask build` | Build the kernel image |
| `cargo xtask run` | Build and boot the kernel in QEMU, serial output on your terminal |
| `cargo test` | Host tests only (no QEMU needed) |
| `cargo xtask test` | Host tests, boot check, in-kernel tests in QEMU and harness self-checks |
| `cargo xtask ci` | Everything CI runs: formatting, clippy and all tests |

A successful boot looks like this:

```
Rhizo 0.1.0
boot time: 1198606 cycles
boot complete
```

QEMU uses KVM automatically when `/dev/kvm` is available and falls back to
emulation otherwise (slower; boot-time numbers are then not meaningful).

## Repository layout

```
kernel/              Bootable kernel image (x86_64-unknown-none)
  src/arch/x86_64/   PVH boot trampoline and hardware drivers
crates/rhizo-hal/    Hardware interfaces (traits) and host test doubles
crates/rhizo-core/   Hardware-independent kernel logic, tested on the host
xtask/               Developer tool: build, run, test, ci
openspec/            Specifications and change proposals (spec-driven development)
AGENTS.md            Instructions for AI coding agents (open standard, any vendor)
CONTRIBUTING.md      How to contribute, with or without AI tools
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE)
