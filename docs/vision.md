# Vision: agent cells beyond the proof of concept

This document records where Rhizo is meant to go after the proof of concept,
so design decisions made now keep that path open. It is not a spec: behavior
still comes from `openspec/specs/`.

## Thesis, restated

AI agents need many short-lived, isolated environments that can be created,
suspended, resumed and forked cheaply. Rhizo makes those operations native
kernel primitives (the *agent cell*) instead of layering them on processes,
containers or whole VMs.

The proof of concept validates the mechanism and its numbers against Linux
processes, Firecracker and gVisor on the same machine. This document covers
what comes next.

## Two markets, one engine

- **B2B (servers).** Density, cost per agent, isolation between tenants,
  and an API/SDK (control plane) to create, fork, suspend and resume cells.
  The value is measurable now; competition is Firecracker, gVisor and cloud
  providers' sandboxes.
- **B2C (local AI).** As models run on personal machines, every user will run
  agents that execute code, install packages and touch files. They need a
  safe place to do it that costs almost nothing when idle. Rhizo becomes the
  agent sandbox of the computer. Same engine, different product around it.

The two are not exclusive: density, security and compatibility are what both
need. We validate on servers (clean numbers) and design for local (AArch64,
compatibility, host integration).

## Local architecture

```
┌──────────────────────────── host (Linux / macOS / Windows) ───────────────┐
│  Rhizo app (ours): VM lifecycle, model runtime, template catalog, agents   │
│                                                                            │
│  Model runtime (any: Ollama, llama.cpp, vLLM…) on the host GPU/NPU        │
│        ▲ local socket (vsock / virtio)                                     │
│  ┌─────┴──────────── Rhizo VM (KVM / Hypervisor.framework / WHPX) ──────┐  │
│  │  cell: agent A   cell: agent B   cell: agent B' (fork)   …           │  │
│  └──────────────────────────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────────────────────────┘
```

- **The model runs next to Rhizo, not inside it.** The user picks any model
  and any runtime; the GPU is used at full speed with no extra layer. The
  boundary is a socket, and the bottleneck is the GPU, not the call.
- **Model weights, KV cache and agent state are different things.**
  - Weights are read-only and shared by everyone: one copy, in the model
    service.
  - The KV cache is per conversation and lives with whoever runs inference.
  - Agent state (program, history, tools, files) is per agent and lives in its
    cell. It is what Rhizo forks, suspends and resumes.
  - Forking an agent mid-conversation does not fork the KV cache unless the
    model service supports prefix caching. Running the model inside the cell
    is a demo of forking heavy state (weights shared copy-on-write), not the
    production design.
- **GPU inside cells (later).** CUDA talks to the host driver through
  `ioctl`s. Rhizo will not ship GPU drivers. A forwarding proxy (as gVisor's
  `nvproxy` does) can let a cell use the host GPU without one.
- **AArch64 first fits local:** Apple silicon, Snapdragon laptops and NVIDIA
  DGX Spark are ARM.

## "Two clicks": the product around the kernel

The kernel is the engine; the app is the product. For B2C the host app is
ours:

1. Starts the Rhizo VM with the host hypervisor, through a light VMM.
2. Detects or installs a model runtime and lets the user pick any model.
3. Offers a **template catalog** (the entry repos): signed snapshots of ready
   environments, such as a Python coding agent, data analysis with numpy, or
   a Node agent.
   - Installing one is a download; starting it is a resume in microseconds.
   - Opening ten agents is ten forks, with no extra memory until they write.
4. Manages agents: list, suspend, resume, clone and delete them.
5. Integrates with the host: shared folders with permissions, network, and
   later the GPU proxy.

A command-line tool (`rhizo run <template>`) comes first. It validates the
flow, and the graphical app builds on it. For B2B the same engine is exposed
as an API/SDK.

## What the kernel must provide for this

| Need | Kernel work |
|---|---|
| Templates as downloads | Snapshots persisted to disk, compressed, deduplicated, signed; lazy restore from disk |
| Run normal software | Dynamic linking, `mmap` of files, C extensions, Node (JIT under W^X) |
| Talk to the host | vsock, shared folders (virtio-fs or similar), networking |
| Laptop friendly | Near-zero CPU and power with idle cells; tickless idle; memory returned to the host |
| Many agents | Density with active cells, bounded p99 latencies, no global locks |
| Untrusted code | Per-cell quotas, KPTI, SMEP/SMAP/PAN, crypto `getrandom`, external audit |
| GPU (later) | Forwarding proxy to the host driver |

## Non-goals

- GPU or NPU drivers in the kernel.
- Real disks or hardware other than virtio for the foreseeable future.
- Running the model inference engine inside Rhizo as the default design.
