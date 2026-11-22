<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/logo/rhizo-logo-dark.svg">
    <img src="docs/logo/rhizo-logo.svg" alt="Rhizo" height="96">
  </picture>
</p>

<h3 align="center">A home for AI agents: they start in an instant, pause and resume like a saved game, clone themselves to try ideas in parallel, and stay sealed off.</h3>

<p align="center">
  <a href="#try-it">Try it</a> ·
  <a href="#get-involved">Get involved</a> ·
  <a href="#the-numbers-that-matter">Numbers</a> ·
  <a href="#where-it-is-going">Roadmap</a> ·
  <a href="docs/overview.md">In depth</a>
</p>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/img/hero-dark.svg">
    <img src="docs/img/hero.svg" alt="The Rhizo vision. A ready-made agent is downloaded from a catalogue and starts in an instant inside a sealed area, talking to your own model (Ollama, llama.cpp or any API). It can be paused and resumed like saving a game, with save slots, and cloned into three branches that try ideas in parallel: the best branch is kept, the others are dropped." width="800">
  </picture>
</p>

AI agents don't just chat any more: they write code, run it, install
packages and open files. Each one needs a room of its own, a safe place to
work that it cannot break out of. Today that room is borrowed from tools built
for something else, so it is slow to open, heavy to keep and costly to copy.

**Rhizo is an operating system kernel built for agents from the ground up.**
Each agent lives in a **cell**. A cell starts in microseconds and a real
agent forked from a warm template answers within a few milliseconds; it sleeps
for almost nothing while the agent waits for its model, can be saved and picked
up later exactly where it was, and can be cloned in an instant so ten copies
try ten ideas at once and you keep the best one. One day you will download
ready-made agents in two clicks and run them with your own model, on your
laptop or by the thousand on a server. Rhizo is open source (MIT) and written
in Rust.

## Who it is for

<table>
<tr>
<td width="50%" valign="top">
<picture><source media="(prefers-color-scheme: dark)" srcset="docs/img/scene-dev-dark.svg"><img src="docs/img/scene-dev.svg" alt="A laptop forks one agent into ten copies; one copy succeeds and is marked with a star." width="100%"></picture>
<b>A developer tries ten strategies at once.</b> Clone the agent at the
moment it has to choose, let every copy try its own fix, keep the one that
passes the tests. The copies share memory until they change it.
</td>
<td width="50%" valign="top">
<picture><source media="(prefers-color-scheme: dark)" srcset="docs/img/scene-company-dark.svg"><img src="docs/img/scene-company.svg" alt="One server holds a grid of a hundred small cells; most are asleep, a few are awake." width="100%"></picture>
<b>A company runs thousands of agents on little hardware.</b> Each customer's
agent is sealed in its own cell; sleeping ones cost a few kilobytes, so one
machine holds ten thousand busy cells.
</td>
</tr>
<tr>
<td width="50%" valign="top">
<picture><source media="(prefers-color-scheme: dark)" srcset="docs/img/scene-hobby-dark.svg"><img src="docs/img/scene-hobby.svg" alt="At home, an assistant in a cell talks to a local model running next to it; it sleeps when idle." width="100%"></picture>
<b>A hobbyist keeps a local assistant.</b> Your model runs on your own GPU;
the assistant lives next to it in a cell, sleeps when you are away, and wakes
in an instant with its files and history.
</td>
<td width="50%" valign="top">
<picture><source media="(prefers-color-scheme: dark)" srcset="docs/img/scene-research-dark.svg"><img src="docs/img/scene-research.svg" alt="A timeline of an experiment with a frozen point; an arrow goes back to it to replay the run." width="100%"></picture>
<b>A researcher freezes and replays an experiment.</b> Freeze an agent run at
any point, save it to a file, and replay it from there, on this machine or
another, as many times as needed.
</td>
</tr>
</table>

## Get involved

- **Try it in minutes**: `rhizo up`, then `rhizo chat` talks to an agent in a
  cell with your own model ([Try it](#try-it)).
- **Build it with us**: good first areas are listed [below](#contribute);
  every change starts as a small spec, so design reviews count as much as
  code.
- **Use it**: running agents for a product, a lab or a hobby? Open an issue
  with your workload; real workloads shape the roadmap.
- **Follow it**: star the repository and watch its releases.

> **Status: proof of concept, measured in CI.** Rhizo already runs CPython,
> Node.js and numpy agents in cells, on Arm and x86, with every figure below
> measured on each change. It is not ready for production, and its external
> security audit is still to come.

## What a cell does

A container shares one kernel among everyone; a microVM gives every agent a
whole operating system. A cell is a third way: one kernel built for agents,
with walls between cells and their life cycle built in.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/img/compare-dark.svg">
    <img src="docs/img/compare.svg" alt="Three stacks side by side. Container: agents on a host Linux kernel shared by every container; starts fast, but the walls are one shared kernel and pausing and cloning are added from outside. MicroVM: each agent on its own guest Linux and VMM over host Linux and KVM; strong walls, but a whole operating system boots per agent and pausing and cloning copy whole VMs. Rhizo cell: many cells on one Rhizo kernel, which creates, pauses, resumes and clones them, over one VMM; walls are the cells' own address spaces, starts take microseconds, and pause and clone are built in." width="800">
  </picture>
</p>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/img/story-dark.svg">
    <img src="docs/img/story.svg" alt="The life of a cell in eight steps: 1 create, an unmodified Linux program becomes a cell; 2 run, with threads, files, network and every CPU; 3 suspend, its whole state frozen in memory; 4 resume where it stopped; 5 fork, copies share memory until they write; 6 freeze as a template, warmed up once to start many; 7 save to a checked file; 8 load it on another machine, where it carries on." width="800">
  </picture>
</p>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/img/memory-dark.svg">
    <img src="docs/img/memory.svg" alt="Memory shared until written. A template holds six pages once. Three forks share all of them; fork 1 has copied one page it wrote, fork 2 two pages. Fork 3 is suspended into a snapshot that keeps only its own copied page and refers to the five shared ones instead of copying them." width="800">
  </picture>
</p>

## The numbers that matter

Measured on every change by `cargo xtask bench`: Rhizo and what it is
compared with run in the same CI job, on the same machine, with the same
clock. The cards come from one run; the text gives the range over recent
runs.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/img/highlights-dark.svg">
    <img src="docs/img/highlights.svg" alt="Six headline figures from one benchmark run. Node.js, ready to answer, forked from a warm template: 4.17 ms (warm fork 4.17 ms, cold start 30.31 ms); with its JIT, cold is 19% faster. numpy from PyPI wheels, forked after import numpy: 9.87 ms (Rhizo cell 9.87 ms, Linux process 9.01 ms); cold: cell 48.60, Linux 44.98 ms. 50 agents on a laptop, 45 of them suspended: 6.6 MB (shared snapshots 6.6 MB, full copies (before) 60 MB); snapshots point at shared pages. A template saved to a file, loaded into a fresh VM: 10.7× sooner (loaded template 1.83 ms, cold agent 19.64 ms); to its first answer (QEMU). Host operation on one cell, how long other cells stall: 14 µs (now 14 µs, every CPU stopped 85 µs); 6× shorter. 10,000 busy cells, 1 vCPU, 99th percentile wait to wake: 0.4–3.7 ms; 39k–46k operations/s." width="800">
  </picture>
</p>

- **Node.js** forked from a warm template answers in about **4 to 7 ms**,
  against **30 to 46 ms** from a cold start; a Linux process cannot fork
  Node.js at all. With its JIT (under write-or-execute pages), a cold start is
  about **16 to 24% faster** than with `--jitless`.
- **numpy** runs **about as fast as in a Linux process** in the same kind of
  VM: a fork after `import numpy` takes 9 to 18 ms in either, and cold
  starts are within about 15% of each other.
- **On a laptop**, 50 agents of one template with 45 suspended take
  **6.6 MB** together, down from **60 MB** before snapshots referred to the
  pages they share.
- **A template saved to a file** and loaded into a fresh VM answers about
  **10 to 12 times sooner** than a cold agent (1.8 to 2.8 ms against 20 to
  28 ms) since the control channel moved to vsock; 2.6 to 4.1 times before.
- **Host operations** on one cell stall the others about **5 to 7 times
  less**: 14 to 22 µs against 80 to 143 µs when every CPU stopped (4 to 5
  times less on Cloud Hypervisor).
- **10,000 busy cells** on one vCPU: the 99th percentile of the wait to wake
  stays between **0.4 and 4.7 ms** depending on the VMM and the run
  (Firecracker the slowest), back to where it was before the cell tree once
  cells that leave no longer cost every other cell a pass over the tree, and
  is gated on main like every other timing.
- For scale: a cell starts a program and runs it to its end in **6.5 to
  7.2 µs**, 63 to 85 times faster than a Linux process in the same VM, and
  forks while holding 50 MiB in **1 to 2 µs**.

<details>
<summary>Every measurement: charts, both architectures, boot time and the full tables</summary>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/img/measured-dark.svg">
    <img src="docs/img/measured.svg" alt="Bar chart, lower is better, bars to scale within each comparison. A cell against a Linux process in the same kind of VM: fork, fork and write 50 MiB, running hello, resume against CRIU, CPython cold and from a warm template, numpy from PyPI wheels cold and from a warm template, a Node.js request from a warm template against cold starts, and MicroPython warm against cold. For scale: resuming a whole Firecracker VM or gVisor sandbox, and running hello in gVisor. The values are in the tables below." width="800">
  </picture>
</p>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/img/arches-dark.svg">
    <img src="docs/img/arches.svg" alt="Bar chart of retired instructions per operation under QEMU -icount, AArch64 next to x86_64: switching between two cells, running hello from start to exit, forking a 50 MiB cell and a glibc program started from a warm template. The values are in the tables below." width="800">
  </picture>
</p>

How the comparisons are made fair, and the thesis targets:
[`docs/overview.md`](docs/overview.md#thesis). Every metric, environment and
baseline: [`bench/README.md`](bench/README.md).

<!-- bench:start -->
Time from launching the VMM to the guest being ready (median, lower is better).
Rhizo prints its ready line at the end of boot; the reference is Firecracker's
CI Linux guest kernel with a minimal `/init` that prints the same line, booted
on the same machine with the same timing method.

<picture><source media="(prefers-color-scheme: dark)" srcset="bench/boot-time-dark.svg"><img src="bench/boot-time.svg" alt="Boot time: Rhizo vs a reference Linux guest"></picture>

| VMM | Rhizo median (ms) | Rhizo p90 (ms) | Linux median (ms) | Linux p90 (ms) | Runs |
|---|---:|---:|---:|---:|---:|
| Firecracker | 5.6 | 6.4 | 144.2 | 150.1 | 20 |
| Cloud Hypervisor | 6.5 | 9.2 | 75.9 | 80.4 | 20 |
| QEMU microvm | 38.4 | 40.4 | — | — | 20 |
| QEMU q35 | 47.7 | 51.4 | 129.0 | 135.8 | 20 |

### A cell against a Linux process, in the same kind of VM

Same machine, same run. Rhizo runs inside Firecracker, and so does the Linux guest of the middle column (the same static programs, as its `/init`): both sides pay the same virtualization. The last column is the same program on the host, one layer of virtualization less. Lazy resume brings pages back on first touch; eager restores every page before the program runs.

| Operation | Rhizo cell | Linux process, same VM | Linux process, host |
|---|---:|---:|---:|
| The `fork` call, holding 50 MiB written | 1.4 µs | 58.8 µs | 1.01 ms |
| Fork, then the child writes all 50 MiB and exits | 1.98 ms | 12.92 ms | 25.30 ms |
| Start a program and run it to its exit (`hello`) | 6.6 µs | 416.9 µs | 104.6 µs |
| Resume a suspended 8 MiB program: lazy / eager (CRIU on the host) | 0.8 µs / 150.1 µs | — | 3.87 ms (CRIU) |

### Whole VMs and sandboxes, for scale

A microVM or a sandbox starts or restores a whole environment (a VM, or a sandbox process with its own kernel), a larger scope than a cell: these figures show the scale, not a like-for-like speed-up. The microVM is a Linux guest in Firecracker (boot to its first output, or a snapshot restored by a new Firecracker process); the gVisor sandbox runs on the host.

| Operation | Rhizo cell | Firecracker microVM | gVisor sandbox |
|---|---:|---:|---:|
| Start a program and run it to its exit (`hello`) | 6.6 µs | 144.17 ms | 61.07 ms |
| Resume a suspended environment until it runs (eager for the cell) | 150.1 µs | 3.47 ms | 87.28 ms |

### A real interpreter

CPython (static, 3.13.18) answering one agent tool call (`cells/python/agent_demo.py`: JSON, regular expressions, dates; the standard library it imports compiled in the cell's own files), in a cell and as a Linux process in the same Firecracker VM:

| Started | Rhizo cell | Linux process |
|---|---:|---:|
| Cold, from its executable (interpreter start, imports, warm-up) | 13.51 ms | 14.66 ms |
| Forked from an interpreter frozen (cell) or kept (process) after the warm-up | 2.48 ms (5.5× faster) | 3.26 ms |

MicroPython (static, v1.24.1) answering one request (`cells/python/rhizo_demo.py`, imported from the cell's own files), in a cell in Firecracker:

| Started | Time |
|---|---:|
| Cold, from its executable (interpreter start, imports, warm-up) | 2.69 ms |
| Forked from a template frozen after the warm-up | 289.9 µs (9.3× faster) |

numpy and its OpenBLAS, installed from their PyPI wheels into a dynamically linked CPython, computing on arrays (`cells/python/numpy_demo.py`: a linear solve, a ufunc, a matrix product), in a cell and as a Linux process in the same Firecracker VM:

| Started | Rhizo cell | Linux process |
|---|---:|---:|
| Cold (interpreter start, `import numpy`, compute) | 48.60 ms | 44.98 ms |
| Forked from an interpreter frozen (cell) or kept (process) after `import numpy` | 9.87 ms (4.9× faster) | 9.01 ms |

Node.js (the official build, dynamically linked, run unmodified with `--jitless`) answering one agent tool call (`cells/node/demo.js`), in a cell and as a Linux process in the same Firecracker VM (Node.js cannot fork itself, so the process has no warm row):

| Started | Rhizo cell | Linux process |
|---|---:|---:|
| Cold, from its executable (`ld.so`, V8 start-up, warm-up) | 30.31 ms | 28.52 ms |
| Forked from Node.js frozen after its warm-up | 4.17 ms (7.3× faster) | — |

### Both architectures, in instructions

Retired instructions per operation, counted under QEMU with `-icount` (the same on every run and every host): x86_64 on `microvm`, AArch64 on `virt`. GitHub's ARM64 runners have no KVM, so AArch64 has no timings from CI.

| Operation | x86_64 | AArch64 |
|---|---:|---:|
| Switch between two cells | 875 | 817 |
| Create the `hello` cell | 28,077 | 21,712 |
| Run `hello` from start to exit | 88,401 | 94,336 |
| Fork a cell holding 50 MiB | 7,010 | 7,696 |
| A static glibc program, cold | 35,246,162 | 36,519,376 |
| The same, forked from a warm template | 76,819 | 73,792 |

Environment: `ci`, KVM: yes, measured 2026-11-21. Regenerate with
`cargo xtask bench --readme`.
<!-- bench:end -->

</details>

## Try it

On a Linux machine with KVM, the shortest path to an agent in a cell talking
to your own model. The first `rhizo up` builds the kernel and CPython for the
cell, which takes a few minutes; after that it starts in a fraction of a
second.

```bash
git clone https://github.com/davidsalvat/rhizo && cd rhizo
sudo apt-get install qemu-system-x86 qemu-system-arm gcc libc6-dev zlib1g-dev
curl -LO https://www.python.org/ftp/python/3.13.18/Python-3.13.18.tar.xz
export RHIZO_CPYTHON_SRC=$PWD/Python-3.13.18.tar.xz
cargo build -p rhizo-cli && alias rhizo=$PWD/target/debug/rhizo

ollama serve & ollama pull qwen2.5:1.5b      # or llama-server, any model with tools
rhizo up
rhizo chat --llm http://127.0.0.1:11434 --model qwen2.5:1.5b
```

Inside the chat, `/fork 2` clones the conversation, `/switch N` moves between
branches and `/suspend` saves the branch for later (`rhizo chat N` picks it up
again). Everything else (the scripted agent, `rhizo run`, `fork`, `freeze`,
`save`, `load`, other VMMs, llama.cpp) is in [`docs/demo.md`](docs/demo.md).
Without a model, `cargo xtask test` boots the kernel and runs every test cell
([building and testing](docs/overview.md#building-and-testing)).

## Under the hood

One Rhizo kernel runs in one VM; cells are unmodified Linux programs above
it. The host drives the kernel with `rhizo` over the VM's virtio socket
device (vsock), or its serial line where it has none, on QEMU, Firecracker
and Cloud Hypervisor alike. A vsock connection is served only once it
presents a secret that the kernel draws at each boot and prints only on its
serial line, which the host keeps in memory and out of its logs. A request
that starts a cell sees the cell's first output in about **0.1 to 0.17 ms**,
now that protocol version 4 returns it with the reply that started the cell,
against 0.2 to 0.25 ms before and 3 to 5 ms when every byte crossed the
serial line. Cells reach the model through the VM's network
card.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/img/architecture-dark.svg">
    <img src="docs/img/architecture.svg" alt="Architecture. On the host machine, Linux with KVM, the rhizo command-line tool talks to the rhizo server, which owns one VM (QEMU, Firecracker or Cloud Hypervisor) and reaches the kernel's control loop over the VM's vsock, authenticated with a per-boot secret (or its serial line); your model runs on the host and is reached through the VM's network card by the sockets layer. In the VM, cells (an agent, a fork, a template, tools) run above the Rhizo kernel, written in Rust: control loop (rhizo-control), Linux system calls (rhizo-abi), sockets (rhizo-net), files (rhizo-fs), memory and copy-on-write (rhizo-mm), scheduler (rhizo-sched), cells, ELF and quotas (rhizo-cell) and snapshots (rhizo-snapshot), over narrow hardware interfaces (rhizo-hal, arch for AArch64 and x86_64, virtio) so the core is tested on the host." width="800">
  </picture>
</p>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/img/hostops-dark.svg">
    <img src="docs/img/hostops.svg" alt="Four CPUs run cells A, B and X over time while the host suspends cell X. Before, every CPU stopped for the operation, so A and B waited too. Now only the two CPUs running X stop, and A and B keep running." width="800">
  </picture>
</p>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/img/snapshot-path-dark.svg">
    <img src="docs/img/snapshot-path.svg" alt="A cell travels as a file. On machine A, rhizo save sends a request with a one-time token over the control channel (vsock, or the serial line); the kernel suspends cell 7 and sends its bytes over TCP through the VM's network card to rhizo, which writes agent.snap ending in a CRC-32C checksum. On machine B, rhizo load asks the kernel to fetch the file the same way; the kernel checks the header, the checksum and every field, caps the quotas, then resumes the cell or forks copies of the template." width="800">
  </picture>
</p>

The kernel is a set of library crates tested on the host, plus a thin
platform layer for each architecture; [`AGENTS.md`](AGENTS.md) has the code
map and [`openspec/specs/`](openspec/specs/) how each part behaves.

## Security

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/img/security-dark.svg">
    <img src="docs/img/security.svg" alt="Everything that reaches the Rhizo kernel from cells, snapshot files and the network is untrusted: hostile cells, hostile snapshot files and hostile network frames. The host's control requests are trusted but decoded defensively; the VMM and hardware are trusted. The kernel defends with privilege levels and own address spaces, quotas, KPTI, E0PD, eager FP/SIMD switching, SMEP, SMAP and PAN, write-or-execute pages, and zeroed memory. Three internal reviews and the fixes since then dealt with 28 findings and fuzzers keep them fixed; an external audit is scoped but not done yet." width="800">
  </picture>
</p>

Rhizo's isolation has been reviewed three times from the inside, and the
reviews and the work since have dealt with 28 findings (27 fixed, one worked
around), with fuzzers and hostile cells in CI that keep them fixed. Timing
side channels other than Meltdown and LazyFP are not covered yet. **No external audit has been done**: its scope, ranked
by risk and estimated at about 39 auditor-days, is ready in
[`docs/audit-scope.md`](docs/audit-scope.md). The threat model, every finding
and what is still open: [`docs/security.md`](docs/security.md).

## Where it is going

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/img/roadmap-dark.svg">
    <img src="docs/img/roadmap.svg" alt="Where Rhizo stands, in nine stages. Done: the foundations (boot in milliseconds on Arm and x86), the cell (ordinary Linux programs, each sealed off), native operations (pause, resume or copy a running cell in microseconds), real programs (threads, networking, Python, every core), proven with numbers (fair comparisons with Linux, VMs and sandboxes, and reviews), a real agent (calls a model, explores branches, pauses between turns) and scale and compatibility (ten thousand busy cells, Node.js, numpy from PyPI). In progress: families and modules. Vision: the product, ready agents in two clicks with your own model." width="800">
  </picture>
</p>

Being built ([`docs/cell-hierarchy.md`](docs/cell-hierarchy.md)): agents
that live in families, each family walled off from the others. Its first
phase is in, but for the last checks of CPU weights and timed benchmarks
on KVM: the cell tree, whose roots each start a domain with its own
identifiers; budgets carved from the parent; a private loopback network
per cell; egress policies checked on every socket; the host places,
re-parents, suspends and kills subtrees; CPUs are shared between domains
by weight with caps per subtree; every tree change is recorded in a
hash-chained audit log; snapshots (format version 11) carry a cell's place
in the tree, and control protocol version 4 speaks it (`rhizo ps --tree`,
`reparent`, `policy`). Channels and handles are being built next. Designed
and coming after: abilities (data analysis,
coding tools, a web browser, documents, your own tools) added to an agent as
child cells from a signed catalogue, as each task needs them.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/img/hierarchy-dark.svg">
    <img src="docs/img/hierarchy.svg" alt="Being built: the cell hierarchy; the tree, its domains, budgets, egress policies, CPU shares by weight and the audit log are in, channels and handles come next. The host is the root; each of its children roots a domain. In domain 1, agent A (egress to the model only, 1 GiB and 8 cells) has two children with no egress, tools and a python module, each reached over a channel. In domain 2, agent B (allowlist egress, 512 MiB and 4 cells) has a worker child. A wall separates the domains. Authority flows down; budgets are carved from the parent; there is no shared loopback, files or counters between domains; egress is checked on every socket and only narrowed, by default none; handles carry rights that only shrink. Timing channels on shared CPUs remain, listed and measured." width="800">
  </picture>
</p>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/img/modules-dark.svg">
    <img src="docs/img/modules.svg" alt="Designed, not built yet. A small static chat agent starts with egress to its model only. Each task that needs an ability adds a child cell, forked from a signed, warm template and reached only over a channel: “Analyse this sheet and chart it” adds data analysis (Python, pandas, plotting), egress: none; “Fix the failing test in my repo” adds coding tools (git, Node.js, a test runner), egress: proxy only; “Check this on a website” adds web browser (headless Chromium), egress: allowlist; “Read these invoices” adds documents (PDF reader, OCR), egress: none; “Update the customer record” adds your own tools (your MCP servers: CRM, database), egress: you set it, within the agent's. The web browser is the sensitive one: the parent or the operator approves it first, and it may reach only allowed sites. Children fork in milliseconds from templates that share their base runtime (measured for CPython and Node.js). Templates are built by a builder cell through rhizo proxy from a lockfile and signed by the operator. Every child talks only to its agent, over a channel, with a budget carved from the agent's and never more egress; what the browser reads stays in that agent's tree." width="800">
  </picture>
</p>

- **Families of cells** (the tree, budgets, private loopback, egress, CPU
  weights and the audit log are built; channels and handles are next). A tree of cells with budgets carved from the parent,
  so no fork bomb escapes them; capability handles whose rights only shrink;
  channels as the only way to talk; a private loopback per cell; egress
  policies per family (none, the model, an allowlist), checked on every
  socket; and an inventory of the 42 shared resources that could leak
  between families, each closed or measured.
- **Abilities as child cells.** An agent starts as a small static chat agent
  that can reach only its model. Each task that needs more adds a child cell,
  forked from a signed, warm template: data analysis (Python with data and
  plotting libraries), coding tools (git, Node.js, a test runner), a web
  browser (headless Chromium: the sensitive one, approved first by the parent
  or the operator and limited to an allowlist of sites), documents (PDF and
  OCR), or your own tools as MCP servers. Each child talks to its agent only
  over a channel, with a budget carved from the agent's and never more egress,
  so what the browser reads cannot leak to another agent's tree. Forking from
  a warm template takes milliseconds for CPython and Node.js today; Chromium is
  not measured yet. Templates share their base runtime, and reach the
  catalogue through a builder cell whose only way out is `rhizo proxy`
  (pinned, hash-checked files) and the operator's signature.
- **The product.** Ready agents to download, an app on your own computer
  (macOS and Windows hosts too), your own model next to it
  ([`docs/vision.md`](docs/vision.md)).

## Contribute

Rhizo is built in the open with spec-driven development: every change starts
as a short proposal under [`openspec/changes/`](openspec/changes/), is
reviewed, then built with its tests and a benchmark. Contributions made with
any AI coding agent, or none, are welcome.

Good first areas:

- **Review the designs** of the cell hierarchy, being built, and of modules
  ([`add-cell-hierarchy`](openspec/changes/add-cell-hierarchy/),
  [`add-cell-modules`](openspec/changes/add-cell-modules/)) before they are
  built.
- **Run your own agent or program in a cell** and report what breaks: a
  missing Linux system call is a well-scoped change.
- **Measure on real hardware**, an ARM64 machine above all
  ([`bench/README.md`](bench/README.md#measuring-on-real-arm64-hardware-dedicated)).
- **Close a listed gap**: TLS for the CPython cell, a virtio-rng driver, a
  disk to save snapshots on VMs without a network card, coverage-guided
  fuzzing ([`docs/security.md`](docs/security.md#known-limitations-and-open-findings)).

Start with [`CONTRIBUTING.md`](CONTRIBUTING.md) (workflow, AI tools) and
[`AGENTS.md`](AGENTS.md) (rules, code map, the feedback loop:
`cargo xtask check` after every edit, `cargo xtask ci` before pushing).

## Learn more

- [`docs/overview.md`](docs/overview.md): the problem, the thesis and its
  targets, everything a cell does today, how the measurements stay fair.
- [`docs/demo.md`](docs/demo.md): agents in cells with your own model.
- [`docs/vision.md`](docs/vision.md): servers and personal computers, one
  engine.
- [`docs/cell-hierarchy.md`](docs/cell-hierarchy.md): families, capabilities,
  containment and modules (designed).
- [`docs/security.md`](docs/security.md) and
  [`docs/audit-scope.md`](docs/audit-scope.md): reviews, findings and the
  external audit's scope.
- [`openspec/specs/`](openspec/specs/): how the system behaves today, one
  spec per capability.
- [`bench/README.md`](bench/README.md): every metric, environment and
  baseline.

## License

[MIT](LICENSE)
