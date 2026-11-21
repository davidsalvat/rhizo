# Proposal

## Why

Rhizo is CPU-only. Agents spend most of their life waiting for a model,
and the model runs on a GPU, an NPU or a hosted API. The user wants Rhizo
to be the best place to run agents **in every situation, not only on
CPU**: GPUs, NPUs and other accelerators, from several vendors, local or
remote.

Today a cell reaches its model only as an ordinary TCP peer (the `model`
egress of `add-cell-hierarchy`). That works, but it throws away what makes
Rhizo different:

- **A fork forgets the model's state.** Ten copies of an agent forked at a
  decision point each send their whole conversation again. Every copy pays
  the prefill of the shared prefix, which for long agent contexts costs
  seconds of GPU time per copy, or tens of seconds on a laptop CPU.
- **A suspended agent loses its KV cache.** Resume, save to a file, load
  on another machine: the engine has evicted or never had the
  conversation's KV, so the first answer after resume pays the full
  prefill again.
- **No budget, no isolation, no audit for accelerators.** Nothing says
  which tree may use which GPU, how much device memory or time it may
  take, or that two domains never share device memory that leaks
  (LeftoverLocals, CVE-2023-4969).
- **A cell cannot run CUDA, ROCm or Metal code itself.** Agents that train
  a small model, run PyTorch or call a vision kernel have no path at all.

The market answers some of this, never all of it at once: Modal snapshots
GPU memory but forks nothing; E2B is CPU-only; GKE Sandbox forwards CUDA
`ioctl`s (nvproxy) but neither forks nor snapshots; RunPod starts
containers faster but keeps no agent state (design D12).

## What Changes

This change is the **design** of accelerators in Rhizo. It holds no code.
The architecture (design D1 to D3):

- **No vendor driver in the Rhizo kernel.** Devices stay with a host-side
  **accelerator broker** (on the host, or in a Linux driver VM on servers
  that must not run vendor drivers on the host). The broker owns the
  vendor stack (CUDA, ROCm, oneAPI, Metal, NPU runtimes) and is the only
  place that touches it. The kernel speaks one vendor-neutral protocol to
  it, over the card first and over virtio-vsock later.
- **Two levels, one protocol.**
  1. **Model sessions** (first, and the default): a cell opens a session on
     a model and sends tokens or text; the broker runs it on an engine
     (llama.cpp, vLLM, SGLang, Ollama, MLX) or forwards it to a hosted API.
     This covers every vendor and every NPU the engines support, and it is
     where fork and resume pay off.
  2. **Compute sessions** (later, opt-in): a cell runs unmodified CUDA or
     Vulkan code through a driver-API remoting library in its files, the
     calls executed by the broker in a context owned by the cell's domain.
- **Accelerators are capabilities.** A cell reaches a device only through a
  handle granted down the tree (the handle model of `add-cell-hierarchy`
  D9), with rights `infer`, `compute` and `stats`. A new cell holds none.
- **Budgets per tree** for device memory, KV bytes in host memory and
  device time, carved from the parent like memory.
- **Fork keeps its meaning.** Forking a cell forks its sessions: the KV is
  shared block by block, copy-on-write, inside the engine where it can be
  (vLLM and SGLang page tables, llama.cpp sequence copy), and through the
  host tier otherwise (D6).
- **Engine-neutral KV tiering** (D7): device memory, then **the cell's own
  frames in the Rhizo VM** (charged to its budget, shared by content only
  within its domain), then the snapshot file. KV blocks travel in a
  neutral container keyed by the model's fingerprint, so a suspended,
  saved or moved cell resumes on any broker with the same model **without
  re-prefill**. Token ids always travel too: re-prefill is the correct
  fallback, never a failure.
- **Hosted model APIs** through the broker acting as the `model` egress
  gateway, with routing aware of prompt caches and cache keys per domain
  (D8).
- **Security** (D9): IOMMU mandatory for any device the broker or a VM is
  given; one domain per device unless a hardware partition (MIG, AMD
  compute and memory partitions, SR-IOV) separates them; device memory and
  local memory scrubbed between domains; prefix caches never shared across
  domains; GPU confidential computing as a production option.
- **Snapshot format version 15** (accelerator section) and **control
  protocol version 5** (`rhizo up --accel`, `rhizo accel`, `rhizo run
  --accel`).

### Phases (design D11)

| Phase | What | Needs real hardware |
|---|---|---|
| A | `rhizo-accel` crate, `Accelerator` link in `rhizo-hal` with a deterministic fake, broker with `fake` and llama.cpp (CPU) engines, model sessions over the card | No |
| B | Session fork, KV in cell frames, snapshot version 15, resume without re-prefill, dedup within a domain | No |
| C | Hosted APIs through the broker, cache-aware routing, per-domain cache keys | No (a fake API server) |
| D | GPU engines (llama.cpp CUDA, Vulkan and Metal, vLLM, SGLang), device ownership per domain, partitions, scrub, device budgets | Yes (self-hosted runner) |
| E | Compute sessions: driver-API remoting, suspend and fork through driver checkpoints | Yes |
| F | virtio-vsock transport, confidential GPUs, attestation | Partly |

### Out of scope

- **Vendor drivers in the Rhizo kernel** and **device passthrough into the
  Rhizo VM** (design D2 explains why; revisited only if a vendor ships an
  open, permissively licensed user-space stack that talks to a documented
  device interface).
- **Training at scale, NCCL and multi-node collectives.** No driver
  checkpoint covers them today.
- **Graphics and display.** Only compute and inference.
- **Sharing one device across domains by time-slicing alone.**

## Decisions

Recorded from the user's review of the design (task 0.2):

- **Architecture:** the host-side broker (D1, D2); no vendor driver in the
  Rhizo kernel.
- **Sharing across domains:** a device or hardware partition serves one
  domain at a time and is scrubbed between domains; the operator may turn
  on an explicit cross-domain override, off by default and audited (D5, D9).
- **Order:** phases A to C start after `add-cell-hierarchy` and
  `add-cell-modules`, which supply the capabilities and budgets they use;
  phases D to F, which need GPU hardware, are decided after phase C.

## Capabilities

### New Capabilities

- `accelerators`: the broker, its protocol, accelerator handles, model and
  compute sessions, budgets, fork, KV tiers, hosted APIs, device ownership
  and scrubbing.

### Modified Capabilities

- `snapshots`: format version 15 with an accelerator section; loaded
  snapshots carry no accelerator authority.
- `isolation`: no storage channel through accelerators or prefix caches
  between domains.
- `control-plane`: protocol version 5, `rhizo up --accel`, `rhizo accel`,
  `rhizo run --accel`.
- `benchmarks`: accelerator metrics.

## Impact

- **New leaf crate `crates/rhizo-accel`**: the protocol (frames, sessions,
  rights), budgets, the KV block container and its content index, tier
  policy. No dependencies; host-tested. Added to `xtask/src/deps.rs` and
  to the code map in `AGENTS.md`.
- `crates/rhizo-hal`: an `AcceleratorLink` trait and a deterministic fake
  device in `testing`.
- `crates/rhizo-abi`: the session endpoint as a channel descriptor under
  `/run/rhizo/accel/`.
- `crates/rhizo-snapshot`: version 15, reading version 14.
- `crates/rhizo-control`: version 5.
- `crates/rhizo-virtio`: virtio-vsock (phase F).
- `cli/`: `rhizo accel` (the broker) with engine adapters; `xtask`: the
  fake broker for tests, a tiny GGUF model for CPU tests.
- Builds on `add-cell-hierarchy` (tree, budgets, handles, channels, egress
  and audit) and must follow it. Its device capabilities, being added there,
  are the handle kind this change uses (design D4).
- No cost on system calls, switches or copy-on-write of ordinary memory.
  The new paths have their own metrics (design D13).
- New host-side dependencies only in the broker (`cli/`), each with its
  license in the design; none in the kernel.
