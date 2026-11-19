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

## Why ARM64

AArch64 is the primary architecture; x86_64 is kept as a second one behind
the same `arch/` boundary (see below). Rhizo's primitives are dominated by
address-space and TLB costs, and AArch64 handles those better:

- **16-bit ASIDs.** Each address space carries one of 65,536 tags (x86_64 has
  4,096 PCIDs), so thousands of cells keep their TLB entries across switches
  instead of flushing them.
- **TLB invalidation broadcast in hardware.** `TLBI ... IS` removes a stale
  translation on every core at once. x86_64 needs inter-processor interrupts
  (TLB shootdowns) for the same thing, which is what copy-on-write `fork` and
  unmapping do all the time.
- **Lighter virtualization.** A dedicated hypervisor level (EL2, with VHE) and
  a simple boot protocol (an image header and a device tree, no firmware, no
  legacy devices) mean smaller VMMs and faster guest entry.
- **Server efficiency.** Graviton, Ampere and Cobalt instances give more cores
  per watt and per dollar, which matters for dense, mostly idle cells.

Why x86_64 stays: GitHub's x86_64 runners are the only place where CI can
measure with KVM today (the ARM64 runners do not expose it), and the sandboxes
Rhizo is compared with (Agent Substrate, Firecracker, E2B) run mostly on
x86_64. Shared code is written once; only the thin `arch/` layer exists twice,
and every spec is tested on both. The trade-offs are in
[`design.md` of the switch-to-arm64 change](openspec/changes/archive/2026-11-19-switch-to-arm64/design.md).

## Thesis

The proof of concept must demonstrate one measurable claim:

> A kernel whose unit is the agent cell can create, suspend, resume and fork
> cells running real Linux programs orders of magnitude faster than today's
> microVM- and userspace-kernel-based sandboxes.

## Performance targets

These are **hypotheses to validate**, not results. The comparison figures are
what each project or its users have published, on different hardware and with
different measurement methods, so they are orders of magnitude to aim at, not
a like-for-like comparison. `cargo xtask bench` produces the like-for-like
numbers (see [Benchmarks](#benchmarks)).

| Metric | Rhizo target | Firecracker | Unikraft | Hyperlight |
|---|---|---|---|---|
| Boot: VMM launch → guest ready (KVM) | **≤ 10 ms** (kernel part ≤ 1 ms) | < 125 ms to a Linux guest accepting API calls[^fc] | < 1 ms guest boot, ~3 ms including the Firecracker VMM[^uk] | sub-millisecond partition creation, no guest OS (reported)[^hl] |
| Resume a suspended cell / VM | **< 100 µs** (in-kernel, no VMM) | 4–28 ms snapshot restore reported[^fcr] | — | ~0.9 ms snapshot restore (hyperlight-unikraft v0.16)[^hlu] |
| Fork a 50 MiB cell | **< 1 ms** (stretch: < 200 µs with lazy copy-on-write) | no native fork (snapshot + restore of a copy) | no native fork in the open-source core | no native fork |
| Idle cell overhead | **a few KiB** plus its own pages | a microVM per sandbox (MiBs) | a microVM per app | a micro-VM per sandbox |

[^fc]: Agache et al., *Firecracker: Lightweight Virtualization for Serverless Applications*, NSDI 2020.
[^uk]: [Unikraft performance documentation](https://unikraft.org/docs/concepts/performance).
[^hl]: [Firecracker vs Microsoft Hyperlight](https://www.pandastack.ai/blog/firecracker-vs-microsoft-hyperlight/) (qualitative, not an official measurement).
[^fcr]: [Benchmarking Firecracker boot and restore](https://numavm.com/blog/2026-03-10-1-second-boot/) and [Restoring uniqueness in microVM snapshots](https://arxiv.org/pdf/2102.12892).
[^hlu]: [hyperlight-unikraft v0.16.0 release notes](https://github.com/hyperlight-dev/hyperlight-unikraft/releases/tag/v0.16.0).

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

| Milestone | Goal | Speed work (each with its benchmark) | Status |
|---|---|---|---|
| H0 | Boot in QEMU, serial console, in-kernel test harness, hardware boundary | — | Done |
| H1 | Physical memory, higher-half paging with W^X, kernel heap, CPU exceptions | Word-level frame bitmap, zero-cost init | Done |
| H1.5 | Fast boot and benchmarks: microvm, Firecracker, Cloud Hypervisor, fast console, release profile, CI gates | 1 GiB direct-map pages (`memory.kernel_pt_frames`, `boot.kernel_cycles`) | Done |
| H1.6 | AArch64 as primary architecture (QEMU `virt`, Firecracker, Cloud Hypervisor), x86_64 kept as second | 1 GiB blocks for the direct map; broadcast TLB invalidation | Done |
| H2 | First cell: user mode, ELF loader, basic Linux syscalls, static binaries | 2 MiB pages for cell memory where possible (`cell.spawn_us`, `cell.run_us`, `cell.pt_frames`) | Done |
| H3 | Multiple cells, scheduler, isolation tests | ASIDs (PCID on x86_64): no TLB flush when switching cells (`cell.switch_ns` vs `cell.switch_untagged_ns`) | Done |
| H4 | Suspend / resume with snapshot-equivalence tests | Lazy restore: snapshot pages mapped on first touch, an in-kernel analogue of Firecracker + `userfaultfd` (`cell.resume_us`, `cell.resume_touch_us`) | — |
| H5 | `fork(n)` with copy-on-write | Lazy copy-on-write including page tables (`cell.fork_us`, `cell.fork_pt_frames`); pre-warmed cell templates (`cell.spawn_from_template_us`) | — |
| H6 | Real workload (MicroPython) with differential tests against Linux | Template of a warm interpreter cell (`cell.spawn_from_template_us`) | — |
| H7 | Reproducible benchmarks against existing sandboxes | Dedicated KVM machine with tight thresholds | — |

Out of scope for the proof of concept: networking, multicore, real disks,
hardware other than virtio/QEMU, full CPython, SDKs.

## Benchmarks

<!-- bench:start -->
Time from launching the VMM to the guest being ready (median, lower is better).
Rhizo prints its ready line at the end of boot; the reference is Firecracker's
CI Linux guest kernel with a minimal `/init` that prints the same line, booted
on the same machine with the same timing method.

![Boot time: Rhizo vs a reference Linux guest](bench/boot-time.svg)

| VMM | Rhizo median (ms) | Rhizo p90 (ms) | Linux median (ms) | Linux p90 (ms) | Runs |
|---|---:|---:|---:|---:|---:|
| Firecracker | 8.0 | 9.3 | 196.3 | 197.3 | 20 |
| Cloud Hypervisor | 11.7 | 20.5 | 105.0 | 109.0 | 20 |
| QEMU microvm | 42.5 | 44.1 | — | — | 20 |
| QEMU q35 | 61.9 | 64.2 | 181.4 | 186.7 | 20 |

Environment: `ci`, KVM: yes, measured 2026-11-19. Regenerate with
`cargo xtask bench --readme`.
<!-- bench:end -->

How benchmarks, environments and baselines work: [`bench/README.md`](bench/README.md).

## Getting started

Prerequisites:

- [rustup](https://rustup.rs). The pinned toolchain (`rust-toolchain.toml`,
  with both kernel targets) is installed automatically on first use.
- QEMU 8.2 or later (verified by CI). AArch64 is the default architecture;
  install the x86_64 emulator too to work on both. On Debian/Ubuntu:

```bash
sudo apt-get install qemu-system-arm qemu-system-x86
```

Commands (add `--arch x86_64` right after `cargo xtask` for the second
architecture):

| Command | What it does |
|---|---|
| `cargo xtask check` | Fast loop, about a second: format, lints, host tests, crate rules |
| `cargo xtask build` | Build the kernel image |
| `cargo xtask run` | Build and boot the kernel (QEMU `virt` on AArch64, `microvm` on x86_64; `--vmm firecracker`, `--vmm cloud-hypervisor`) and report the time to ready |
| `cargo xtask test` | Host tests, boot check, in-kernel tests and harness self-checks |
| `cargo xtask bench` | Release-build boot benchmarks on every available VMM, against a reference Linux guest |
| `cargo xtask ci` | Everything CI runs for one architecture |

A successful boot on AArch64 (QEMU `virt`, emulated) looks like this:

```
Rhizo 0.1.0
memory: 126 MiB usable, 126 MiB free, 9 page-table frames
rhizo-ready cycles=1114569
boot time: 1114569 cycles (17833 us)
boot complete
```

QEMU uses KVM automatically when `/dev/kvm` is available and the host CPU has
the kernel's architecture, and falls back to emulation otherwise (timings are
then not meaningful). Firecracker and Cloud Hypervisor need KVM; `xtask`
downloads pinned releases on first use.

## Repository layout

```
kernel/                Bootable kernel image
  src/arch/aarch64/    Image header and EL1 entry, MMU, vectors, PL011/16550, PSCI
  src/arch/x86_64/     PVH entry, paging, IDT/GDT, serial and debug port
crates/rhizo-hal/      Hardware interfaces (traits) and host test doubles
crates/rhizo-core/     Boot reports, options, device tree, exception reports
crates/rhizo-mm/       Frames, page tables (both entry formats), heap
xtask/                 Developer tool: check, build, run, test, bench, ci
openspec/              Specifications and change proposals (spec-driven development)
.agents/skills/        Project skills (open Agent Skills format)
AGENTS.md              Instructions for AI coding agents (open standard, any vendor)
CONTRIBUTING.md        How to contribute, with or without AI tools
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE)
