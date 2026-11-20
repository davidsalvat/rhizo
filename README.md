<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/logo/rhizo-logo-dark.svg">
    <img src="docs/logo/rhizo-logo.svg" alt="Rhizo" height="96">
  </picture>
</p>

<p align="center"><b>An operating system kernel built for AI agents.</b></p>

AI agents write and run code. Each one needs a safe place to do it: a sandbox
it cannot escape, that starts instantly, costs nothing while the agent waits
for the model, and can be paused, resumed or copied to try several ideas at
once. Today that place is built from pieces made for something else (virtual
machines, containers, processes), and it shows: slow to start, heavy to keep,
expensive to copy.

Rhizo is an operating system kernel built for that job. Its basic unit is the
**agent cell**, a sealed environment that the kernel itself can create,
suspend, resume and fork in microseconds. It is meant for two kinds of users:

- **platforms that run many agents on servers**, where density, cost per agent
  and isolation between customers matter;
- **people running agents on their own computer**, next to a local model,
  where an agent should cost almost nothing while idle.

Underneath, Rhizo is deep: it rebuilds memory, scheduling and isolation around
the agent. On top, the goal is that none of it shows: two clicks and your
agents are ready, fast and safe. Rhizo is open source (MIT) and written in
Rust.

> **Status: pre-alpha proof of concept.** Nothing here is usable in production
> yet. See [where it stands](#status).

## The problem

An AI agent that runs code today sits on top of a tall stack. Every layer was
designed for something else, and what agents need most (fast suspend and
resume, cheap clones of the whole environment, strict isolation of untrusted
code) is bolted on from the outside.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/img/stack-dark.svg">
    <img src="docs/img/stack.svg" alt="Today: orchestrator, host Linux, a VMM or userspace kernel and often a guest Linux under the agent. With Rhizo: the orchestrator talks to the Rhizo kernel, which runs the agents as cells and offers create, suspend, resume and fork." width="800">
  </picture>
</p>

Agent workloads look nothing like classic services:

| Classic service | AI agent |
|---|---|
| Long-lived, always on | Short bursts, mostly idle (waiting for a model or a human) |
| Trusted code | Untrusted, model-generated code |
| Tens per node | Thousands per node, most of them asleep |
| Scaling means more replicas | What matters is suspending and resuming quickly |
| Static permissions | Per-task permissions: tools, network, secrets |

Rhizo explores what happens when those needs are the design center of the
kernel itself. The name comes from the *rhizome*: an underground stem that
branches into independent shoots (fork), can regrow a whole plant from a
fragment (snapshot) and lies dormant until it sprouts again (suspend/resume).

## Vision

Rhizo is complex underneath so that it can be simple on top. The kernel
rebuilds the foundations of the operating system around the agent; the person
using it sees none of that.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/img/layers-dark.svg">
    <img src="docs/img/layers.svg" alt="Layers of Rhizo. On top, what the user sees: a two-click app and a template catalog, both planned, and their agents. A line separates what the user does not see: agent cells, created, suspended, resumed and forked in microseconds (measured), and below them the Rhizo kernel, which rebuilds memory, scheduling, isolation, networking and snapshots." width="800">
  </picture>
</p>

For someone running agents, it should look like this. The steps that exist
today show their measured time; the others are planned:

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/img/clicks-dark.svg">
    <img src="docs/img/clicks.svg" alt="The user's path in five steps: pick a template and download it (both planned), open an agent (a resume from memory, measured in microseconds), open ten more (ten forks, no extra memory until they write, measured), and suspend them to come back later (a few kilobytes per idle agent, measured)." width="800">
  </picture>
</p>

On a personal computer the model runs next to Rhizo, not inside it: any model
and any runtime, at full GPU speed. Agents live in cells and talk to the model
over a local socket.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/img/model-dark.svg">
    <img src="docs/img/model.svg" alt="Local architecture. On the user's computer, a model runtime such as Ollama, llama.cpp or vLLM runs on the GPU or NPU with one shared copy of the weights. The Rhizo VM (on KVM today; macOS and Windows planned) runs agent cells, including a fork, each keeping its own state. Cells reach the model over a local socket, and the Rhizo app coordinates both; the app and the socket are planned." width="800">
  </picture>
</p>

The same engine serves servers and personal computers:

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/img/markets-dark.svg">
    <img src="docs/img/markets.svg" alt="Two markets, one engine. Servers: many agents per machine, low cost per agent, an API to create, fork and resume. Personal computers: a two-click app, downloadable templates, your own local model. Both products are planned; both run on the Rhizo kernel and its agent cells, which exist today." width="800">
  </picture>
</p>

The full picture, and what the kernel still needs for it, is in
[`docs/vision.md`](docs/vision.md).

## Status

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/img/roadmap-dark.svg">
    <img src="docs/img/roadmap.svg" alt="Where Rhizo stands, in eight stages. Done: the foundations (boot in milliseconds on Arm and x86), the cell (ordinary Linux programs, each sealed off), native operations (pause, resume or copy a running agent in microseconds), real programs (threads, networking, Python, every core) and proven with numbers (fair comparisons with Linux, VMs and sandboxes, and a security review). In progress: a real agent. Next: scale and compatibility. Vision: the product, with downloadable agents and your own model." width="800">
  </picture>
</p>

Each stage groups several pieces of work, and each piece ships its speed work
with a benchmark that proves it; the details live in the archived changes
under [`openspec/changes/archive/`](openspec/changes/archive/).

Out of scope for the proof of concept: real disks, hardware other than
virtio/QEMU, SDKs.

## What a cell can do

A cell runs an unmodified static Linux program (Rust or C with glibc) through
a Linux-compatible system call layer. Its whole state (memory, registers,
files) belongs to the kernel, so the kernel can stop it, copy it and start it
again:

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/img/lifecycle-dark.svg">
    <img src="docs/img/lifecycle.svg" alt="A static Linux executable is created as a running cell. The cell can be suspended into a snapshot and resumed, forked into copy-on-write children, or frozen as a template from which new, already warmed-up cells start. Times are in the tables below." width="800">
  </picture>
</p>

- **Suspend / resume.** A snapshot is a versioned, self-contained image of the
  cell. Resume is lazy: pages come back on first touch.
- **Fork(n).** Children share the parent's pages, and even its page tables,
  until one of them writes.
- **Templates.** Freeze a cell after its warm-up, then start new cells from it
  instead of from the executable.
- **Threads.** `pthread_create`, mutexes, condition variables and futexes;
  snapshots, forks and templates carry every thread.
- **Network.** Pipes, `poll`/`select`/`epoll`, Unix and TCP/UDP sockets
  between cells, and a virtio-net card (smoltcp) to reach the outside;
  established connections are reset on suspend, resume and fork, as CRIU
  does by default.
- **Multi-core.** Cells run in parallel on up to 16 CPUs, each on one CPU
  at a time; the threads of one cell still share that CPU, and system calls
  of different cells take turns.
- **CPython.** Unmodified CPython 3.13 (static) runs as a cell, its standard
  library in the cell's own files; a template frozen after its imports
  answers each request without starting the interpreter again.
- **Isolation, reviewed.** An adversarial review of the cell boundary, with
  hostile cells and property tests that keep it fixed:
  [`docs/security.md`](docs/security.md) lists the threat model, the
  findings and what is still open.

## Thesis

The proof of concept must demonstrate one measurable claim:

> A kernel whose unit is the agent cell can create, suspend, resume and fork
> cells running real Linux programs orders of magnitude faster than today's
> microVM- and userspace-kernel-based sandboxes.

A cell is born inside a kernel that is already running, so it is closer to a
process than to a VM. The measurements below therefore come in two tiers: a
cell against a Linux process doing the same operation in the same kind of VM
(the like-for-like gain, and the smaller one), and whole microVMs and
sandboxes, shown for scale. Resume is shown lazy (pages come back on first
touch) and eager (every page restored first).

The targets are hypotheses the proof of concept must validate. Where they stand:

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/img/scorecard-dark.svg">
    <img src="docs/img/scorecard.svg" alt="The three thesis targets, all met: fork of a 50 MB cell under 1 ms, resume from memory under 100 µs, a few KB per idle cell. Measured values are in the tables below." width="800">
  </picture>
</p>

<details>
<summary>Targets next to the figures other sandboxes publish</summary>

The comparison figures are what each project or its users have published, on
different hardware and with different measurement methods, so they are orders
of magnitude to aim at, not a like-for-like comparison. The like-for-like
numbers are [below](#measured).

| Metric | Rhizo target | Firecracker | Unikraft | Hyperlight |
|---|---|---|---|---|
| Boot: VMM launch → guest ready (KVM) | **≤ 10 ms** (kernel part ≤ 1 ms) | < 125 ms to a Linux guest accepting API calls[^fc] | < 1 ms guest boot, ~3 ms including the Firecracker VMM[^uk] | sub-millisecond partition creation, no guest OS (reported)[^hl] |
| Resume a suspended cell / VM | **< 100 µs** (in-kernel, no VMM) | 4–28 ms snapshot restore reported[^fcr] | — | ~0.9 ms snapshot restore (hyperlight-unikraft v0.16)[^hlu] |
| Fork a 50 MiB cell | **< 1 ms** (stretch: < 200 µs with lazy copy-on-write) | no native fork (snapshot + restore of a copy) | no native fork in the open-source core | no native fork |
| Idle cell overhead | **a few KiB** plus its own pages | a microVM per sandbox (MiBs) | a microVM per app | a micro-VM per sandbox |

</details>

[^fc]: Agache et al., *Firecracker: Lightweight Virtualization for Serverless Applications*, NSDI 2020.
[^uk]: [Unikraft performance documentation](https://unikraft.org/docs/concepts/performance).
[^hl]: [Firecracker vs Microsoft Hyperlight](https://www.pandastack.ai/blog/firecracker-vs-microsoft-hyperlight/) (qualitative, not an official measurement).
[^fcr]: [Benchmarking Firecracker boot and restore](https://numavm.com/blog/2026-03-10-1-second-boot/) and [Restoring uniqueness in microVM snapshots](https://arxiv.org/pdf/2102.12892).
[^hlu]: [hyperlight-unikraft v0.16.0 release notes](https://github.com/hyperlight-dev/hyperlight-unikraft/releases/tag/v0.16.0).

## Measured

`cargo xtask bench` measures Rhizo and the existing mechanisms in the same run,
on the same machine, with the same clock:

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/img/measured-dark.svg">
    <img src="docs/img/measured.svg" alt="Bar chart, lower is better, bars to scale within each comparison. A cell against a Linux process in the same kind of VM: fork, fork and write 50 MiB, running hello, resume against CRIU, CPython cold and from a warm template, and MicroPython warm against cold. For scale: resuming a whole Firecracker VM or gVisor sandbox, and running hello in gVisor. The values are in the tables below." width="800">
  </picture>
</p>

The charts and the tables below come from the same run (the date is under the
tables). Shared runners are noisy, so figures move between runs; publishable
numbers will come from a dedicated machine. How benchmarks, environments and baselines work:
[`bench/README.md`](bench/README.md).

<details>
<summary>Boot time and the full comparison table</summary>

<!-- bench:start -->
Time from launching the VMM to the guest being ready (median, lower is better).
Rhizo prints its ready line at the end of boot; the reference is Firecracker's
CI Linux guest kernel with a minimal `/init` that prints the same line, booted
on the same machine with the same timing method.

<picture><source media="(prefers-color-scheme: dark)" srcset="bench/boot-time-dark.svg"><img src="bench/boot-time.svg" alt="Boot time: Rhizo vs a reference Linux guest"></picture>

| VMM | Rhizo median (ms) | Rhizo p90 (ms) | Linux median (ms) | Linux p90 (ms) | Runs |
|---|---:|---:|---:|---:|---:|
| Firecracker | 8.3 | 9.1 | 192.3 | 195.0 | 20 |
| Cloud Hypervisor | 6.5 | 13.5 | 97.0 | 106.8 | 20 |
| QEMU microvm | 44.1 | 44.7 | — | — | 20 |
| QEMU q35 | 63.0 | 67.0 | 183.4 | 186.5 | 20 |

### A cell against a Linux process, in the same kind of VM

Same machine, same run. Rhizo runs inside Firecracker, and so does the Linux guest of the middle column (the same static programs, as its `/init`): both sides pay the same virtualization. The last column is the same program on the host, one layer of virtualization less. Lazy resume brings pages back on first touch; eager restores every page before the program runs.

| Operation | Rhizo cell | Linux process, same VM | Linux process, host |
|---|---:|---:|---:|
| The `fork` call, holding 50 MiB written | 1.4 µs | 87.2 µs | 856.5 µs |
| Fork, then the child writes all 50 MiB and exits | 6.03 ms | 24.16 ms | 22.78 ms |
| Start a program and run it to its exit (`hello`) | 7.3 µs | 534.1 µs | 98.0 µs |
| Resume a suspended 8 MiB program: lazy / eager (CRIU on the host) | 1.3 µs / 588.6 µs | — | 2.37 ms (CRIU) |

### Whole VMs and sandboxes, for scale

A microVM or a sandbox starts or restores a whole environment (a VM, or a sandbox process with its own kernel), a larger scope than a cell: these figures show the scale, not a like-for-like speed-up. The microVM is a Linux guest in Firecracker (boot to its first output, or a snapshot restored by a new Firecracker process); the gVisor sandbox runs on the host.

| Operation | Rhizo cell | Firecracker microVM | gVisor sandbox |
|---|---:|---:|---:|
| Start a program and run it to its exit (`hello`) | 7.3 µs | 192.25 ms | 70.40 ms |
| Resume a suspended environment until it runs (eager for the cell) | 588.6 µs | 3.67 ms | 97.17 ms |

### A real interpreter

CPython (static, 3.13.18) answering one agent tool call (`cells/python/agent_demo.py`: JSON, regular expressions, dates; the standard library it imports compiled in the cell's own files), in a cell and as a Linux process in the same Firecracker VM:

| Started | Rhizo cell | Linux process |
|---|---:|---:|
| Cold, from its executable (interpreter start, imports, warm-up) | 18.15 ms | 18.29 ms |
| Forked from an interpreter frozen (cell) or kept (process) after the warm-up | 3.71 ms (4.9× faster) | 5.52 ms |

MicroPython (static, v1.24.1) answering one request (`cells/python/rhizo_demo.py`, imported from the cell's own files), in a cell in Firecracker:

| Started | Time |
|---|---:|
| Cold, from its executable (interpreter start, imports, warm-up) | 3.59 ms |
| Forked from a template frozen after the warm-up | 498.4 µs (7.2× faster) |

Environment: `ci`, KVM: yes, measured 2026-11-20. Regenerate with
`cargo xtask bench --readme`.
<!-- bench:end -->

</details>

## Try it

You need [rustup](https://rustup.rs) (the pinned toolchain installs itself)
and QEMU 8.2 or later; the C test cells also need gcc with a static glibc.
On Debian/Ubuntu:

```bash
sudo apt-get install qemu-system-arm qemu-system-x86 gcc libc6-dev
```

Boot the kernel, run the in-kernel tests and every cell (AArch64, the default):

```bash
cargo xtask test
```

The same on the second architecture:

```bash
cargo xtask --arch x86_64 test
```

Benchmarks (Firecracker and Cloud Hypervisor need KVM; `xtask` downloads
pinned releases on first use):

```bash
cargo xtask bench
```

```bash
cargo xtask --arch x86_64 bench
```

`cargo xtask check` is the fast loop (format, lints, host tests, crate rules)
and `cargo xtask ci` runs everything CI runs. AArch64 is the primary
architecture: 16-bit ASIDs and TLB invalidation broadcast in hardware suit
cells that switch and fork all the time. x86_64 stays because GitHub's x86_64
runners are where CI can measure with KVM; only the thin `arch/` layer exists
twice, and every spec is tested on both
([trade-offs](openspec/changes/archive/2026-11-19-switch-to-arm64/design.md)).

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

## Learn more

- [`openspec/specs/`](openspec/specs/): how the system behaves today, one spec
  per capability.
- [`docs/vision.md`](docs/vision.md): where Rhizo goes after the proof of
  concept (servers and local AI, one engine) and what the kernel needs for it.
- [`bench/README.md`](bench/README.md): every metric, environment and baseline.
- [`AGENTS.md`](AGENTS.md): code map, rules and the feedback loop (for people
  and AI coding agents alike).
- [`CONTRIBUTING.md`](CONTRIBUTING.md): how to contribute, with or without AI
  tools.

## License

[MIT](LICENSE)
