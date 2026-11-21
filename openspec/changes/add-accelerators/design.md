# Design

This change designs how cells use GPUs, NPUs and other accelerators. It
holds no code. It builds on `add-cell-hierarchy` (tree, budgets, handles,
channels, egress, audit) and `add-cell-modules` (capabilities as child
cells), and must be implemented after their phase 2.

Figures marked **(derived)** are computed here from the cited numbers;
every other figure cites its source in [Sources](#sources).

## Goals

- **G1. Every situation.** One design for a laptop with an integrated GPU
  or an NPU, a workstation with one discrete GPU, a server with eight, and
  a hosted model API. Several vendors: NVIDIA, AMD, Intel, Apple,
  Qualcomm, and NPUs.
- **G2. Rhizo's semantics survive.** Create, fork, suspend, save, load
  and move keep working for a cell that uses an accelerator, and stay
  cheap where they are cheap today.
- **G3. No re-prefill.** A forked, resumed or moved agent does not
  recompute the model state of a conversation it already had, whatever
  engine serves it.
- **G4. Safe by construction.** No ambient access to a device, budgets per
  tree, no storage channel between domains through device memory or
  caches, IOMMU always.
- **G5. Testable without GPUs.** CI has no GPU. Every piece that can be
  tested on the host is, and the rest is isolated behind one interface.
- **G6. Rules of the project.** No GPL code (rule 1), the kernel core
  never touches hardware (rule 2), speed first with benchmarks (rules 4,
  7, 8), cell state serializable (rule 6).

## Non-goals

- GPU or NPU drivers inside the Rhizo kernel (consistent with
  `docs/vision.md`, "Non-goals"; D2 re-examines it with numbers).
- Graphics, display, video encoding.
- Multi-node training and collectives (NCCL, RCCL): no driver checkpoint
  covers them ([CRIUgpu](https://arxiv.org/html/2502.16631v1) states NCCL
  is unsupported; [Dynamo Snapshot](https://developer.nvidia.com/blog/nvidia-dynamo-snapshot-fast-startup-for-inference-workloads-on-kubernetes/)
  lists multi-GPU as future work).

## Facts that shape the design

| Fact | Figure | Source |
|---|---|---|
| PCIe Gen5 x16 | about 64 GB/s per direction, theoretical (128 GB/s both ways) | [NVIDIA H100](https://www.nvidia.com/en-us/data-center/h100/) |
| NVLink 4 (H100 SXM) | 900 GB/s total, about 450 GB/s per direction | [Hopper in depth](https://developer.nvidia.com/blog/nvidia-hopper-architecture-in-depth/) |
| NVLink-C2C (GH200) | 900 GB/s total between CPU and GPU | [NVIDIA H100](https://www.nvidia.com/en-us/data-center/h100/) |
| H100 SXM memory | 3.35 TB/s HBM3 | [NVIDIA H100](https://www.nvidia.com/en-us/data-center/h100/) |
| KV per token, Llama 3.1 8B, bf16 | 2 × 32 layers × 8 KV heads × 128 × 2 B = **128 KiB** | (derived, from the model's configuration) |
| KV per token, Llama 3.1 70B, bf16 | 80 layers: **320 KiB** | (derived) |
| KV per token, Qwen2.5 1.5B (README demo), bf16 | 28 layers × 2 KV heads: **28 KiB** | (derived) |
| CRIUgpu, Llama 3.1 8B on H100 | 54 GB of GPU state, checkpoint 77 s, restore 39 s | [CRIUgpu](https://arxiv.org/html/2502.16631v1) |
| CRIUgpu, GPT-2 small, 1 × A100 | 9.94 GB in 12.8 s, about **0.78 GB/s** (derived) | [CRIUgpu](https://arxiv.org/html/2502.16631v1) |
| Dynamo Snapshot, gpt-oss-120b | 129 GiB: upstream CRIU restore 119 s, optimized 15 s (about **8.6 GiB/s**, derived); weights split out: under 5 s, 21× faster start | [Dynamo Snapshot](https://developer.nvidia.com/blog/nvidia-dynamo-snapshot-fast-startup-for-inference-workloads-on-kubernetes/) |
| Dynamo Snapshot, KV released before checkpoint | Qwen3-0.6B on B200: 190 GiB checkpoint to 6 GiB | same |
| Modal GPU memory snapshots | vLLM Qwen2.5-0.5B cold start 45 s to 5 s; up to 10× | [Modal](https://modal.com/blog/gpu-mem-snapshots) |
| API remoting latency budget | 5 to 20 µs per call keeps most inference under 5% overhead | [Network requirements for API remoting](https://arxiv.org/abs/2401.13354) |
| H100 confidential computing | under 7% average overhead, near zero on 70B | [H100 CC benchmark](https://arxiv.org/html/2409.03992v2) |
| LeftoverLocals | about 5.5 MB leaked per kernel launch on a Radeon RX 7900 XT, 181 MB per llama.cpp query | [Trail of Bits](https://blog.trailofbits.com/2024/01/16/leftoverlocals-listening-to-llm-responses-through-leaked-gpu-local-memory/) |
| NVLink covert channel | over 70 kbit/s between two GPUs, across GCP VMs | [NVBleed](https://arxiv.org/html/2503.17847v1) |
| Shared prompt caches at API providers | 7 of 17 providers shared caches globally (2025) | [Auditing prompt caching](https://arxiv.org/abs/2502.07776) |
| CachedAttention | 87% lower TTFT, 7.8× prefill throughput, 70% lower cost on multi-turn | [CachedAttention](https://arxiv.org/abs/2403.19708) |
| LMCache | 1.9× to 8.1× lower TTFT at low load, up to 15× throughput | [LMCache](https://arxiv.org/abs/2510.09665) |
| Mooncake | 115% and 107% more requests on A800 and H800 clusters | [Mooncake, FAST '25](https://www.usenix.org/conference/fast25/presentation/qin) |

Two consequences recur below:

- **Moving KV beats recomputing it by an order of magnitude.** An 8,192
  token context of Llama 3.1 8B is 1 GiB of KV (derived). Over PCIe Gen5
  at about 50 GB/s in practice (an assumption, below the 64 GB/s
  theoretical figure) it loads in about **21 ms**. Recomputing it takes
  about 2 × 8·10⁹ × 8192 ≈ 1.3·10¹⁴ FLOP, about **0.27 s** on an H100 at
  half of its 990 dense BF16 TFLOPS (derived). On a laptop CPU serving
  Qwen2.5 1.5B at a few hundred prompt tokens per second, the same
  context takes tens of seconds to recompute, against 224 MiB of KV read
  from RAM in milliseconds (derived).
- **Device memory has no copy-on-write, but paged KV does.** vLLM's
  PagedAttention keeps a reference count per KV block and copies only the
  block being written ([PagedAttention](https://arxiv.org/abs/2309.06180));
  SGLang's radix tree and llama.cpp's sequence copy share the same way.
  The engine already has fork for KV. Rhizo only has to ask for it.

## D1. How a cell reaches an accelerator

Three families were compared, with the vendor-neutral boundary each needs.

### (a) Device passthrough into the Rhizo VM, driver in the kernel

The VMM hands the PCI function (a whole GPU, an SR-IOV virtual function or
a MIG-backed vGPU) to the Rhizo VM through VFIO and the IOMMU; the Rhizo
kernel runs a driver; cells use it through `ioctl`s on device files.

- **VMM support.**
  - QEMU: VFIO, mature.
  - Cloud Hypervisor: VFIO with `--device path=/sys/bus/pci/devices/<BDF>/`,
    GPUDirect P2P for Turing to Hopper; snapshots and live migration need
    a variant driver with VFIO migration v2 (generic `vfio_pci`, used for
    GPUs, has none), and device state moves only during downtime
    ([Cloud Hypervisor VFIO](https://github.com/cloud-hypervisor/cloud-hypervisor/blob/main/docs/vfio.md)).
  - Firecracker: no GPU passthrough in a release. VFIO commits landed in
    2026 with NVMe as the tested device and snapshots disallowed while a
    VFIO device is attached; GPU passthrough is the stated goal of its PCIe
    roadmap ([PCIe roadmap PR #4894](https://github.com/firecracker-microvm/firecracker/pull/4894),
    [issue #849](https://github.com/firecracker-microvm/firecracker/issues/849)).
  - Apple Hypervisor.framework (the laptop target): no PCI passthrough at
    all.
- **Pros.** Native speed, no host software beyond the VMM; partitions
  (SR-IOV VFs, MIG) map naturally to VMs.
- **Cons.**
  - A driver in Rhizo per vendor and per generation (D2): not realistic.
  - The VM's snapshot and migration are blocked or slowed by the device
    (Firecracker refuses snapshots; Cloud Hypervisor needs migration v2).
    Rhizo's suspend and fork are per cell, inside the VM, so they would
    still need a device checkpoint path in the kernel.
  - Excludes macOS hosts and Firecracker today.
- **Numbers.** Zero added latency per call. Effort: years per vendor.

### (b) API remoting: cells call a broker that owns the device

The device stays with the host (or a driver VM). A cell's calls are
forwarded over a channel to a broker that executes them.

Three cut points exist:

1. **Kernel `ioctl` level** (gVisor's nvproxy): forward `/dev/nvidia*`
   `ioctl`s to the host driver. nvproxy needs a table per driver version
   (unknown versions refuse to start), supports only `compute` and
   `utility` by default, not `/dev/nvidia-caps` (MIG), and gives "no
   hardware-level isolation beyond what the NVIDIA kernel-mode driver
   provides" ([gVisor GPU](https://gvisor.dev/docs/user_guide/gpu/)).
   It works in gVisor because the sentry and the driver share one host
   kernel and address space. Across a VM boundary, `ioctl`s carry guest
   addresses (UVM, pinned host memory mapped into the GPU) that the host
   cannot follow without mediation, so this cut is fragile for Rhizo.
   For DRM drivers the mediation exists upstream: **virtio-gpu DRM native
   context** forwards the amdgpu, Intel and freedreno UAPI to the host
   with host blobs (QEMU pull of March 2026, virglrenderer 1.2, host Linux
   6.13) ([QEMU patch](https://patchew.org/QEMU/20260306195048.2869788-1-alex.bennee@linaro.org/20260306195048.2869788-13-alex.bennee@linaro.org),
   [Collabora](https://www.collabora.com/news-and-blog/blog/2025/01/15/the-state-of-gfx-virtualization-using-virglrenderer/)).
2. **Driver API level** (rCUDA): a library in the cell replaces
   `libcuda.so.1` (which the CUDA runtime loads with `dlopen`, so programs
   stay unmodified) and forwards `cu*` calls. The driver API is stable
   across driver versions, unlike `ioctl`s. Cost: one round trip per
   synchronous call; asynchronous calls batch. 5 to 20 µs per round trip
   keeps most inference under 5% overhead
   ([API remoting](https://arxiv.org/abs/2401.13354)); slower links need
   PCIe-class interconnects ([rCUDA study](https://riunet.upv.es/bitstream/10251/67332/1/reano_cluster2013_autor.pdf)).
   vAccel does the same at a framework level over virtio and vsock, with
   QEMU and Firecracker ([vAccel](https://github.com/nubificus/virtio-accel)).
3. **Inference level**: forward requests to an inference engine
   (OpenAI-style HTTP today). Vendor-neutral by construction: every
   engine and every hosted API speaks it.

- **Pros.** No driver in Rhizo; works on every VMM (the card or vsock is
  enough), on macOS hosts, and for remote GPUs; the broker can checkpoint
  with vendor tools (`cuda-checkpoint`, KFD CRIU ioctls).
- **Cons.** A latency per call (level 2), a broker to keep current with
  drivers (level 1, avoided by levels 2 and 3), the broker is trusted.

### (c) A model-serving cell that owns the device

A cell (a module, in `add-cell-modules` terms) owns the device and serves
other cells through channels and capabilities.

- **Pros.** Fits the capability model exactly; the server is forkable and
  suspendable like any cell.
- **Cons.** The owning cell needs the device, which brings back (a): a
  driver in Rhizo. Without one, the "cell" can only be a front for the
  host broker, which is (b) with an extra hop.

### Decision

**(b) at levels 3 and 2, presented to cells as (c).** The device belongs
to a host-side **accelerator broker**; the Rhizo kernel never touches it.
To a cell, the broker appears through capabilities exactly like a serving
cell: a handle granted down the tree, opening channels under
`/run/rhizo/accel/`. Inference-level **model sessions** come first
(phases A to D); driver-API **compute sessions** come later (phase E);
`ioctl` forwarding is rejected for CUDA and kept as an option for DRM
drivers through virtio-gpu native context (D14). Passthrough into the
Rhizo VM is rejected (D2).

## D2. Why no vendor driver in the Rhizo kernel

| Vendor | Kernel driver and license | User space a cell would need | Verdict |
|---|---|---|---|
| NVIDIA | Open kernel modules, dual MIT/GPL; required for Blackwell and later; depend on the GSP firmware, which is a closed blob ([NVIDIA README](https://download.nvidia.com/XFree86/Linux-x86_64/595.84/README/kernel_open.html)) | `libcuda` (closed), which expects the Linux `ioctl` ABI of the same driver version | The MIT side could be ported, but it is written against Linux kernel interfaces, is very large, and must track every release; the closed user space would still need the exact Linux ABI. Not realistic. |
| AMD | `amdgpu`, MIT ([license fix](https://lore.kernel.org/all/20221121172058.694436-1-alexander.deucher@amd.com/T/)); KFD for compute | ROCm (mostly permissive) | Legally possible, practically a second Linux DRM stack. Reachable from the host through DRM native context instead. |
| Intel | `i915` and `xe`, MIT-style DRM code | Level Zero, oneAPI | Same as AMD. |
| Apple | Asahi `drm/asahi` (Rust), dual GPL-2.0/MIT, reverse-engineered ([patch](https://patchwork.kernel.org/project/dri-devel/patch/20230307-rust-drm-v1-18-917ff5bc80a8@asahilina.net/mbox)) | Mesa | Only on bare-metal Linux on Apple silicon; under macOS (the actual laptop host) there is no passthrough. Metal on the host only. |
| NPUs (Apple ANE, Qualcomm Hexagon, Intel NPU, AMD XDNA) | Mixed; AMD's XDNA driver ships under a custom license file ([xdna-driver](https://github.com/amd/xdna-driver)) | Core ML, QNN, OpenVINO, Ryzen AI | Only reachable through vendor runtimes on the host. |

Even with a permissive driver, the user space each vendor ships expects
Linux device files and `ioctl`s byte for byte, and its firmware is closed.
A driver in Rhizo would also break rule 2 (the core runs in the host
simulator) unless it lived entirely behind the HAL, which it cannot,
being the hardware. The broker removes all of this: it is ordinary Linux
or macOS software using each vendor's supported stack.

**What remains possible later.** If an accelerator appears with an open
user space that talks to a documented, permissively licensed device
interface, a driver could live in a `kernel/src/arch`-style platform
module behind `AcceleratorLink` (D4), with the same protocol above it.
Nothing in this design prevents it.

## D3. The broker and its protocol

```text
 host (Linux, macOS) or Linux driver VM            Rhizo VM
┌──────────────────────────────────────────┐   ┌──────────────────────────────┐
│ rhizo accel (broker)                     │   │ kernel                       │
│  ├ engines: fake, llama.cpp, vLLM,       │◄──┤  rhizo-accel: sessions,      │
│  │  SGLang, Ollama, MLX                  │   │  budgets, KV index, tiers    │
│  ├ hosted APIs (the `model` gateway)     │   │  AcceleratorLink (card,      │
│  ├ compute executor (phase E)            │   │  then virtio-vsock)          │
│  └ devices: whole, MIG, CPX, VFs         │   │ cells: /run/rhizo/accel/<l>  │
└──────────────────────────────────────────┘   └──────────────────────────────┘
```

- **Where it runs.**
  - *Laptop and workstation:* on the host, next to the user's engine
    (the README's `ollama serve` or `llama-server` become broker engines).
  - *Multi-tenant server:* in a Linux **driver VM** that receives the GPUs
    (or their partitions) through VFIO, so the host runs no vendor driver;
    one driver VM per tenant group when partitions are given to different
    customers.
  - *Remote:* across the network, as hosted APIs are; only level 3.
- **Transport.** Phase A uses the existing virtio-net card to a host
  address, as the model endpoint already does (the kernel's own socket,
  exempt from cell egress like save and load in `add-cell-hierarchy` D7).
  Phase F adds virtio-vsock, which avoids the TCP stack and works on
  QEMU, Firecracker and Cloud Hypervisor. Both carry the same frames.
- **Frames.** Versioned, length-prefixed, little endian, like the control
  protocol: `Open{model fingerprint, rights, budget}`, `Append{tokens}`,
  `Generate{params, max}`, `Fork{session}`, `Export{session, range}`,
  `Import{blocks}`, `Close`, and for phase E `Call{api, args, buffers}`.
  The broker answers with tokens, block descriptors and charges (device
  time and memory used), which the kernel accounts per cell.
- **Who talks to the broker.** Only the kernel. A cell never gets a
  socket to the broker: it writes to a channel endpoint, the kernel checks
  rights and budget, tags the frame with the cell's domain and session,
  and forwards it. The broker therefore sees domains, never cell-chosen
  identities.
- **The model fingerprint** is the SHA-256 of the weights file set plus
  the tokenizer and the KV layout (layers, heads, head size, dtype,
  rotary parameters). Two brokers with the same fingerprint can exchange
  KV blocks (D7).

## D4. Crates, HAL and what runs on the host

The hardware-boundary rule (AGENTS.md rule 2) requires the core to run in
the deterministic host simulator. The accelerator is outside the VM, so
the kernel's only "hardware" is a message link:

```rust
/// A link to an accelerator broker: request frames out, completions in,
/// and the regions of guest memory a request lends to the broker.
pub trait AcceleratorLink {
    /// Queues one request frame; `false` if the link has no room now.
    fn send(&mut self, frame: &[u8]) -> bool;
    /// Hands the next completion frame to `take`; `false` if none waits.
    fn recv(&mut self, take: &mut dyn FnMut(&[u8])) -> bool;
}
```

- **`rhizo-hal`** gains `AcceleratorLink` and, in `testing`, a
  **`FakeAccelerator`**: a deterministic broker in memory with a toy
  model whose next token is a hash of the context, KV blocks whose bytes
  are a function of (fingerprint, position, token), a device memory
  limit, a prefill cost in fake device ticks proportional to the tokens
  computed, and injectable failures (device lost, out of memory, broker
  restart). It makes "no re-prefill" observable: the fake counts the
  tokens it prefilled.
- **New leaf crate `rhizo-accel`** (no dependencies): frames and their
  checked decoding, sessions and their rights, budgets (carved like
  `rhizo-cap`'s), the KV block container, the content index per domain,
  the tier policy (what to evict to frames, what to drop), the routing
  key for hosted APIs. All host-tested, with property tests (budgets
  never exceed the parent, an exported then imported session generates
  the same tokens, dedup never crosses domains).
- **`rhizo-abi`**: the session endpoint is a channel descriptor
  (`AF_UNIX` `SOCK_SEQPACKET`, as in `add-cell-hierarchy` D10) at
  `/run/rhizo/accel/<label>`, so unmodified programs use `read`, `write`,
  `poll` and `epoll`. A small OpenAI-compatible shim in the cell (a
  module, `add-cell-modules`) lets existing agent code use
  `http://localhost:…/v1` on the cell's own loopback.
- **`kernel`**: the platform link (the card's socket, then vsock) and the
  wiring. No device code.
- **`cli/`** (host only): `rhizo accel`, the broker, with engine adapters
  (HTTP and engine-specific KV endpoints); the only place with new
  dependencies (each justified in its task, permissive licenses only).
- **`xtask`**: runs the fake broker for in-QEMU tests (like the echo
  server) and fetches a tiny GGUF model so llama.cpp runs **on the CPU in
  CI**, which tests a real engine's slot save and restore without a GPU.

| Piece | Host-testable | In-QEMU without GPU | Needs a GPU |
|---|---|---|---|
| `rhizo-accel` protocol, budgets, KV container, index, tiers | Yes | — | — |
| `FakeAccelerator` sessions, fork, export, import | Yes | Yes | — |
| Kernel forwarding, rights, `/run/rhizo/accel`, snapshots v15 | Partly (`rhizo-abi`, `rhizo-snapshot`) | Yes (fake broker) | — |
| llama.cpp adapter, slot save and restore, sequence copy | — | Yes (CPU build, tiny model) | — |
| Hosted API gateway and routing | Yes (fake API server) | Yes | — |
| vLLM, SGLang, LMCache adapters | — | vLLM's CPU backend is possible but slow; nightly | Yes |
| Partitions, scrubbing, device budgets enforced by hardware | — | — | Yes |
| Compute sessions (`libcuda` remoting, `cuda-checkpoint`) | Protocol only | — | Yes |

## D5. Handles, budgets, sharing and scheduling

**Handles.** An accelerator is an object a handle names, in the model of
`add-cell-hierarchy` D9 (rights only reduced, moved only over channels,
revocable). `add-cell-hierarchy` is gaining device capabilities in a
parallel edit; this change uses that handle kind and only defines its
rights:

- `infer`: open model sessions on the models the handle lists;
- `compute`: open compute sessions (phase E);
- `stats`: read the device's use by this handle's subtree.

The host grants a root a handle with `rhizo run --accel gpu0:infer`; a
parent passes attenuated copies to children. A new cell holds none. A
module child (`add-cell-modules`) that runs a model gets one only if its
agent grants it.

**Budgets**, carved from the parent's and returned at the end, exactly
as memory budgets in `add-cell-hierarchy` D4 (reserved only):

| Budget | Unit | Enforced by |
|---|---|---|
| `accel.mem` | bytes of device memory (KV and compute allocations) | the broker, per domain context; the kernel keeps the tree's ledger |
| `accel.kv_host` | bytes of KV in cell frames | the kernel: they are ordinary frames charged to the cell's memory |
| `accel.time` | device milliseconds per second (a share) | the broker's scheduler |
| `accel.sessions` | open sessions | the kernel |

**Sharing a device.** The options:

| Mechanism | Isolation of memory and faults | Granularity | Verdict for Rhizo |
|---|---|---|---|
| Time-slicing (contexts in turn) | Separate address spaces, no fault isolation, no bandwidth isolation; GKE does not recommend it between pods ([GKE Sandbox](https://docs.cloud.google.com/kubernetes-engine/docs/concepts/sandbox-pods)) | Any | **Within a domain only** |
| MPS | Separate address spaces in newer versions, shared L2 and memory bandwidth, a fault in one client can reset all ([MPS vs MIG study](https://arxiv.org/html/2604.22430v2)) | Any | Within a domain only |
| NVIDIA MIG | Separate SMs, L2 slices and memory paths per instance ([MIG guide](https://docs.nvidia.com/datacenter/tesla/mig-user-guide/620/supported-mig-profiles.html)); timing channels across instances remain ([USENIX Security '26](https://www.usenix.org/system/files/conference/usenixsecurity26/sec26_prepub_gu-cheng.pdf)) | Up to 7 fixed profiles | **Across domains** |
| AMD compute and memory partitions | CPX gives 8 logical GPUs (one per XCD), NPS4 four memory partitions of 48 GB on MI300X ([AMD](https://instinct.docs.amd.com/projects/amdgpu-docs/en/develop/gpu-partitioning/mi300x/overview.html)) | Fixed modes | Across domains |
| SR-IOV virtual functions | VF isolated from VFs and the PF (MI300 MxGPU, 8 VFs) ([AMD](https://rocm.blogs.amd.com/software-tools-optimization/compute-memory-modes/README.html)) | Fixed | Across domains (one VF per driver VM) |
| One engine batching many sessions | One process, shared KV pool | Per request | Within a domain only, with prefix caches salted per domain |

**Rule:** a device, or a hardware partition of it, belongs to at most one
domain at a time. Inside a domain, every session of every cell may share
one engine, which batches them continuously; this is where a tree of
agents gets its throughput, since continuous batching of 10 agents costs
little more GPU time than one. Across domains, sharing needs a hardware
partition. The operator can override this on a single-user machine
(`rhizo up --accel gpu0 --shared-across-domains`), which is audited and
reported by `rhizo accel ls` as a covert channel, as exclusive CPU sets
are optional in `add-cell-hierarchy` D17.

**Scheduling.** Two levels:

- The broker schedules domains on their device or partition (none to do
  when partitions are exclusive), and inside a domain enforces each
  subtree's `accel.time` share with deficit round robin over sessions,
  charging the measured device time back.
- The kernel scheduler is unchanged: a cell waiting for tokens blocks on
  its channel like on any socket, and costs nothing while it waits.

## D6. Fork, suspend and snapshot of accelerator state

### Model sessions (phases A to D)

A session's state is: the model fingerprint, the token ids so far, the
sampling state (seed and position, so a fork can choose to diverge or
not), and the KV for those tokens.

- **Fork.** Forking a cell forks each of its sessions with one `Fork`
  frame. The broker asks its engine for a block-shared copy:
  - vLLM: a new request whose prefix hits the parent's cached blocks (the
    block hash chain), reference counts only; the last partial block is
    copied on write ([PagedAttention](https://arxiv.org/abs/2309.06180)).
  - SGLang: the radix tree shares the prefix node.
  - llama.cpp: sequence copy inside the unified KV cache.
  - Cost: metadata only, independent of context length; the target is
    **under 1 ms per session** (`accel.session_fork_us`).
  - When the engine cannot share (another device, no room), the broker
    falls back to export from the parent and import into the child
    (D7): about 21 ms per GiB over PCIe Gen5, 2.4 ms per GiB over NVLink
    at 450 GB/s, under 1 ms per GiB inside one device at HBM speed
    (derived), still far below re-prefill.
- **Suspend.** The cell's sessions stay open in the broker until memory
  pressure or the tier policy evicts them; evicted KV moves to the
  cell's frames (D7). Resume finds them in the engine or imports them.
- **Save, load, move.** The snapshot carries the accelerator section:
  sessions (fingerprint, tokens, sampling state) and references to the
  KV frames, which the existing snapshot machinery stores and shares like
  any page. A loaded cell on another machine re-imports its KV into the
  new broker if it has the same fingerprint, or re-prefills otherwise.

### Compute sessions (phase E)

Device memory written by arbitrary kernels has no copy-on-write: the
broker cannot know which buffers a kernel writes, because pointers hide
inside arguments and structures. Allocation-level sharing is therefore
unsound except for memory mapped read-only.

- **Weights and other read-only data** are mapped read-only into every
  context that uses them, through the CUDA virtual memory API (export a
  shareable handle, map with read access in each context) ([CUDA VMM](https://docs.nvidia.com/cuda/cuda-driver-api/group__CUDA__VA.html)).
  This is what Dynamo's GPU Memory Service does to keep weights out of
  checkpoints (74 of 129 GiB for gpt-oss-120b). Fork shares them.
- **Everything else** is checkpointed by the driver: the broker locks
  the context (`cuCheckpointProcessLock`), checkpoints device memory to
  host memory, and releases the device ([CUDA checkpointing](https://docs.nvidia.com/cuda/cuda-driver-api/group__CUDA__CHECKPOINT.html));
  on AMD the KFD CRIU ioctls do the same ([CRIUgpu](https://arxiv.org/html/2502.16631v1)).
- **Fork of a compute session** = checkpoint of the parent's private
  device memory into host memory **once**, then **lazy restore**: each
  copy gets the image only when it next issues a call. The image in host
  memory is shared copy-on-write by the copies on the host side, so ten
  copies of a 2 GiB private state cost 2 GiB of host memory until they
  run. A copy that never touches the GPU again never pays the restore.
- **Cost.** Locking took 240 to 500 ms and GPU-state checkpoint 4.9 to
  28 s in CRIUgpu's GPT-2 runs; Dynamo's optimized restore reaches about
  8.6 GiB/s. A compute-session fork is therefore **hundreds of
  milliseconds to seconds**, against microseconds for a CPU-only cell.
  That is stated, measured (`accel.compute_fork_ms`) and not hidden.
- **Restrictions** inherited from the drivers: same GPU type and count on
  restore, no NCCL, specific driver branches.

## D7. Engine-neutral KV and model-state tiering

### Tiers

```text
tier 0: device memory      (engine's pool, per domain)      µs
tier 1: the cell's frames  (Rhizo VM RAM, charged to cell)   ms per GiB over PCIe
tier 2: the snapshot file  (disk, shared pages by reference) tens of ms per GiB from NVMe
```

- **Tier 1 is the cell's own memory.** KV blocks evicted from the engine
  are written into frames the kernel allocates for the cell, mapped into
  no address space, charged to its memory budget (`accel.kv_host`). The
  broker writes them by DMA from the device into guest RAM the VMM
  exposes to it (vhost-style shared memory), so there is no extra copy
  on the host. Because they are frames, the existing machinery applies
  unchanged: they are shared copy-on-write by forks, carried by
  snapshots, saved to files, and shared by reference between snapshots
  of one template (`snapshots-on-disk`).
- **Shared by content within a domain only.** The kernel indexes KV
  blocks by (domain, fingerprint, hash chain of the block's tokens, as
  vLLM's prefix cache does). Two cells of one domain with the same prefix
  share one frame. Cells of different domains never do, even with
  identical prompts: deduplication across domains is a storage channel
  (a write that is slow because it broke sharing reveals the other
  domain's prompt), exactly as page deduplication would be
  (`add-cell-hierarchy` D12).
- **Tier 2** is the snapshot file: KV frames are pages like others.

### The neutral container

A KV block in tiers 1 and 2 is engine-neutral:

```text
KvBlock = { fingerprint: [u8; 32], first_token: u32, tokens: [u32; N],
            layout: { layers, kv_heads, head_dim, dtype, block_tokens },
            data: per layer K then V, row-major, little endian }
```

Each engine adapter converts between this and the engine's native form
on export and import:

| Engine | Export | Import | Notes |
|---|---|---|---|
| llama.cpp | slot save (`POST /slots/{id}?action=save`, needs `--slot-save-path`) or sequence state API | slot restore | Works on CPU, Vulkan, CUDA, Metal; tested in CI on CPU ([llama.cpp tutorial](https://github.com/ggml-org/llama.cpp/discussions/13606)) |
| vLLM | KV connector (LMCache), chunked | same | LMCache's chunked load beats vLLM's own CPU offload ([LMCache](https://arxiv.org/abs/2510.09665)) |
| SGLang | HiCache storage backend: `get`, `exist`, `set` ([HiCache](https://lmsys.org/blog/2025-09-10-sglang-hicache)) | same | The broker implements the three calls on top of tier 1 |
| Ollama, MLX | none stable | none | Re-prefill fallback; the adapter can wrap llama.cpp for Ollama models |
| Hosted APIs | impossible (provider-side) | impossible | Tokens and cache keys only (D8) |

Conversion is possible only between engines with the same fingerprint:
the same weights, precision and KV layout. llama.cpp's quantized KV
(`q8_0`) and vLLM's bf16 KV are different layouts, so moving between them
re-prefills. **The token ids always travel**, so re-prefill is always a
correct fallback and never an error.

### Compared with existing KV systems

| System | Tiers | Granularity | Engine | What Rhizo takes |
|---|---|---|---|---|
| CachedAttention (AttentionStore) | HBM, host DRAM, SSD, per conversation; layer-wise preload, async save, scheduler-aware eviction | Session | Own | Sessions as the unit, preload by layer, hints from the scheduler (the kernel knows which cells are about to run) |
| LMCache | CPU, local disk, remote disk, Redis; zero-copy between tiers | Chunk (configurable) | vLLM, SGLang | Its connector as the vLLM adapter |
| Mooncake | Disaggregated pool over the cluster's DRAM, SSD and NICs; prefill and decode separated | Block, cluster-wide | vLLM, SGLang | Not the pool: Rhizo keeps KV with the cell that owns it. Its transfer engine is a candidate transport between machines |
| SGLang HiCache | GPU, host, pluggable storage | Radix node | SGLang | The 3-call backend interface |
| Dynamo KVBM | GPU, CPU, disk through NIXL | Block | Dynamo backends | Same idea at cluster scale |

The difference: these systems store KV **for the engine**, keyed by
content, in a pool shared by all users. Rhizo stores KV **with the cell**
that owns it, so it follows the cell through fork, suspend, save, load and
move, is charged to its budget, and never crosses a domain. Inside one
engine, the engine's own prefix cache still does its job within a domain.

## D8. Hosted model APIs

The `model` egress of `add-cell-hierarchy` D7 names a host-side gateway.
The broker is that gateway when `rhizo up --accel` is used:

- **Same session interface.** A cell opens a session on `claude-…`,
  `gpt-…` or any OpenAI-compatible endpoint exactly as on a local model;
  the broker holds the API keys, which never enter the VM.
- **Cache-aware routing.** Providers cache prompt prefixes for a time
  (Anthropic: 5 minutes by default or 1 hour, reads at 0.1× the input
  price, writes at 1.25× or 2×; OpenAI: explicit breakpoints with 30
  minute or 12 hour lifetimes) ([OpenRouter caching guide](https://openrouter.ai/docs/guides/best-practices/prompt-caching),
  [sticky routing](https://openrouter.ai/blog/tutorials/prompt-caching-sticky-routing/)).
  The broker:
  - routes every session of a fork family to the same provider, region
    and key, so ten forks reuse one cached prefix instead of writing ten;
  - places cache breakpoints at the fork point, which the kernel reports;
  - picks the cache lifetime from the cell's state: a suspended cell
    with a resume expected within the hour gets the long lifetime, a
    short-lived fork the default;
  - re-sends nothing a provider has cached when a cell resumes within
    the lifetime.
- **Per-domain cache keys.** Provider caches shared across customers
  leaked prompts in 7 of 17 providers in 2025
  ([Gu et al.](https://arxiv.org/abs/2502.07776)). The broker derives the
  provider's cache or session key from (operator secret, domain), so
  two domains never land in one cache namespace through Rhizo, and
  uses separate API keys per domain when the operator configures them.
- **Snapshots carry tokens, not provider state.** A resumed or moved
  cell sends its history again; the provider's cache makes that cheap
  when the key and lifetime still match.

## D9. Security

- **IOMMU mandatory.** The broker refuses a device whose DMA is not
  behind an IOMMU (VFIO with an IOMMU group on Linux, the platform's
  DART on Apple silicon). A driver VM gets its GPU only through VFIO with
  every function of the IOMMU group (Cloud Hypervisor's rule). Guest RAM
  lent to the broker for KV transfers is limited to the frames of the
  request (`AcceleratorLink` lends regions per request).
- **One domain per device or hardware partition** (D5). The broker binds
  a device or partition to a domain when the first session of that domain
  opens and keeps it until the domain's last session closes.
- **Scrub between domains.** Before a device or partition changes
  domain, the broker clears device memory it allocated and runs a
  clearing kernel over local (shared) memory and registers where the
  vendor has not fixed it: LeftoverLocals leaked about 5.5 MB per kernel
  launch on AMD, Apple, Qualcomm and Imagination GPUs, enough to rebuild
  another process's llama.cpp output
  ([LeftoverLocals](https://leftoverlocals.com), [paper](https://arxiv.org/abs/2401.16603)).
  The KV frames of tier 1 are kernel frames and are zeroed on free like
  any frame.
- **Prefix caches salted per domain.** Engine prefix caches make hits
  measurably faster, which leaks prompts by timing
  ([InputSnatch](https://arxiv.org/abs/2411.18191)). Inside one domain
  this is accepted (one trust domain); across domains, engines are
  separate (D5), and if the operator shares one engine anyway, every
  request carries a per-domain salt (vLLM `cache_salt`, which has had
  bypass bugs such as [CVE-2026-105752](https://access.redhat.com/security/cve/CVE-2026-105752);
  the broker therefore prefers separate engines).
- **Side channels that remain** across hardware partitions: MIG timing
  channels ([USENIX Security '26](https://www.usenix.org/system/files/conference/usenixsecurity26/sec26_prepub_gu-cheng.pdf)),
  NVLink contention at over 70 kbit/s ([NVBleed](https://arxiv.org/html/2503.17847v1)),
  virtualized GPU side channels ([NDSS](https://www.ndss-symposium.org/?p=23666)).
  They join the covert-channel inventory of `add-cell-hierarchy` D12 as
  "accepted and measured", with `covert.accel_bps` measured on real
  hardware between two MIG instances. For more, give each domain a whole
  GPU and no NVLink peer in another domain.
- **The broker is trusted** and is the largest new attack surface: it
  parses frames from the kernel (never from cells directly), with checked
  decoding in `rhizo-accel` shared by both sides and fuzzed. It runs
  unprivileged except for its device files; on servers it runs in the
  driver VM, so a broker or driver compromise stays out of the host.
- **Confidential computing** (phase F, production option). H100 CC mode
  passes one GPU to a confidential VM with an encrypted bounce buffer and
  attestation of the GPU's firmware; overhead under 7%
  ([benchmark](https://arxiv.org/html/2409.03992v2),
  [NVIDIA](https://developer.nvidia.com/blog/confidential-computing-on-h100-gpus-for-secure-and-trustworthy-ai/)).
  For Rhizo: the driver VM is the confidential VM that owns the GPU, the
  Rhizo VM is another, and the link between them is encrypted and bound
  to both attestations. Compute-session checkpoints in CC mode stay
  inside the driver VM.
- **Audit.** Grants and revocations of accelerator handles, device
  binding to domains, scrubs, overrides and budget refusals go to the
  audit log of `add-cell-hierarchy` D18.

## D10. Snapshots and the control plane

- **Snapshot format version 15** adds an accelerator section: for each
  session, its label, model fingerprint, token ids, sampling state and
  the list of KV frames (by reference, like pages). Version 14 files read
  as having no sessions. Loaded snapshots carry no accelerator authority:
  sessions are restored only if the host binds an accelerator handle to
  the label (`rhizo load file --bind-accel chat=gpu0`), as channels are
  bound in `add-cell-hierarchy` D13; otherwise the session reads as
  closed and its KV frames are dropped.
- **Control protocol version 5**:
  - `rhizo up --accel SPEC` starts or connects the broker (`fake`,
    `llama:URL`, `vllm:URL`, `sglang:URL`, `api:NAME`) and lists devices;
  - `rhizo accel ls` shows devices, partitions, their domains, budgets
    and use;
  - `rhizo run --accel gpu0:infer --accel-mem 8G --accel-time 50%`;
  - `rhizo grant accel` and `rhizo revoke` through the handle commands of
    `add-cell-hierarchy`.

## D11. Phases and CI

| Phase | Content | CI without GPU | Exit criterion |
|---|---|---|---|
| A | `rhizo-accel`, `AcceleratorLink` with `FakeAccelerator`, the broker with `fake` and llama.cpp (CPU, tiny GGUF model), model sessions over the card, handles and budgets | All | An agent cell chats through `/run/rhizo/accel/chat` on both architectures and every VMM; a cell without a handle cannot |
| B | Session fork, KV in cell frames, snapshot version 15, save and load across `rhizo down`/`up` without re-prefill, dedup within a domain | All | The fake and llama.cpp count **zero** prefilled tokens on resume and on fork |
| C | Hosted APIs through the broker, routing, per-domain keys | All, against a fake API server that models a prefix cache | Ten forks write one cache entry |
| D | GPU engines (llama.cpp CUDA, Vulkan, Metal; vLLM with LMCache; SGLang with HiCache), domain binding, partitions, scrub, enforced device budgets | Host and fake parts; GPU jobs on a self-hosted runner, nightly | LeftoverLocals-style probe reads only zeros across domains |
| E | Compute sessions: `libcuda` remoting library, executor, read-only weight sharing, `cuda-checkpoint` suspend and lazy fork | Protocol only | PyTorch inference in a cell; fork and resume measured |
| F | virtio-vsock link, confidential GPUs, attestation | vsock on QEMU when the CI host has `vhost-vsock` | `accel.token_rtt_us` lower over vsock than over the card |

Each phase that is to be archived on its own is first split into its own
change, as `add-cell-hierarchy` D16 does.

## D12. Where this puts Rhizo against the market

| | Device access | Fork of an agent with its model state | Suspend and resume of model state | Isolation between tenants on one GPU | Vendors |
|---|---|---|---|---|---|
| **Modal** | gVisor with GPUs (T4 to B200) ([Modal](https://modal.com/resources/best-sandboxes-streaming-code-execution-ai-apps)) | No | GPU memory snapshots of a function (alpha), up to 10× faster cold start | Whole GPUs | NVIDIA |
| **E2B** | None: CPU-only, Firecracker ([E2B FAQ](https://docs.e2b.dev/faq/gpu-support)) | No | No | — | — |
| **GKE Sandbox (gVisor, nvproxy)** | `ioctl` forwarding, CUDA only, version-locked ([gVisor](https://gvisor.dev/docs/user_guide/gpu/)) | No | No | MIG or whole GPU; time-sharing not recommended | NVIDIA |
| **RunPod serverless** | Containers on GPUs; FlashBoot pre-loads images: sub-200 ms claimed for small models, 91 to 324 s for a 32B model on two H200 ([RunPod](https://www.runpod.io/blog/cut-vllm-cold-starts-runpod-serverless)) | No | No | Whole GPUs | NVIDIA, AMD |
| **NVIDIA Dynamo Snapshot** | Kubernetes workers | No | Worker-level, under 5 s for gpt-oss-120b (prototype) | — | NVIDIA |
| **Rhizo (this design)** | Broker: any engine or hosted API, compute later | **Yes: sub-millisecond per session in one engine, KV shared block by block** | **Yes: KV in the cell's snapshot, no re-prefill on any broker with the model** | Hardware partitions across domains, batching inside one | Any the engines support, NPUs included |

Rhizo does not beat these on raw kernel throughput: it uses the same
engines. It wins on what agents do: branch, pause and move a
conversation without paying its prefill again, with budgets and
isolation per tree.

## D13. Benchmarks

New metrics in `cargo xtask bench` (rule 7), each with its gate decided
when its phase lands:

| Metric | What | Phase |
|---|---|---|
| `accel.token_rtt_us` | One generated token through the cell's endpoint against the same engine called directly from the host (the overhead of Rhizo's path) | A |
| `accel.session_open_us` | Open a session on a loaded model | A |
| `accel.session_fork_us` | Fork a cell with one session of 8,192 tokens | B |
| `accel.resume_prefill_tokens` | Tokens prefilled to resume a saved cell (must be 0 on a matching broker) | B |
| `accel.kv_restore_ms_per_gib` | Tier 1 to tier 0 import | B (fake, llama.cpp CPU), D (GPU) |
| `accel.kv_shared_bytes` | Host memory of 10 forks of one session against 10 copies | B |
| `accel.api_cache_writes` | Provider cache writes for 10 forks (fake API: must be 1) | C |
| `accel.compute_fork_ms` | Fork of a compute session with 1 GiB of private device memory | E |
| `covert.accel_bps` | Timing channel between two domains on two MIG instances | D |

CPU paths are untouched: no accelerator code runs on system calls,
switches, ordinary forks or loopback, so their gated metrics must not
move.

## D14. Alternatives rejected or deferred

- **A driver in Rhizo with passthrough** (option a): rejected for now
  (D2). Deferred, not excluded, for an accelerator with an open user
  space and a documented interface.
- **nvproxy-style `ioctl` forwarding for CUDA**: rejected. A table per
  driver version, and guest addresses in `ioctl`s cannot cross the VM
  boundary safely without mediating every structure; the driver API is
  the stabler cut.
- **virtio-gpu DRM native context** for AMD, Intel and Qualcomm compute
  (Vulkan, so llama.cpp's Vulkan backend, inside a cell): deferred. It
  would need virtio-gpu in `rhizo-virtio` and DRM UAPI forwarding in
  `rhizo-abi`, is QEMU-only (Firecracker has no virtio-gpu), and every
  engine it enables is reachable through the broker already.
- **A global KV pool like Mooncake**: rejected as the default; it shares
  across tenants and decouples KV from the cell, which breaks fork,
  snapshot and the domain rule. Usable as the transport of tier 2 between
  machines of one domain.
- **Time-slicing a GPU across domains**: rejected without the operator's
  override (D5).
- **Running the engine inside a cell** (a CPU build of llama.cpp in a
  cell already works): kept as a demo and for CPU-only machines, not as
  the GPU design.

## D15. Risks and open questions

- **Broker maintenance.** Engines change their KV APIs often (LMCache
  connectors, llama.cpp slot endpoints). Mitigation: adapters are small,
  tested against pinned versions, and re-prefill is always correct.
- **Compute-session fidelity.** Driver-API remoting must cover the calls
  PyTorch makes, including `cuMemMap` and graphs; gaps show up as errors
  in the cell, not as corruption. Effort is the largest of all phases.
- **Checkpoint support from vendors** (`cuda-checkpoint` driver branches,
  AMD KFD capabilities) can change; compute-session fork depends on it.
- **DMA into guest RAM** needs VMM support for sharing guest memory with
  the broker (memfd-backed RAM on QEMU and Cloud Hypervisor, vhost-user);
  without it, KV crosses with one extra copy.
- **Open:** whether `accel.time` should allow pooled shares among
  siblings (today budgets are reserved only, `add-cell-hierarchy`
  decision 1).

## Decision table

Gains are for agents that use a model. "CPU-only" is Rhizo today: a cell
calls a model as a plain TCP peer.

| Option | Gain vs current CPU-only Rhizo | Gain vs the market (Modal, E2B, GKE gVisor nvproxy, RunPod) | Risk | Effort |
|---|---|---|---|---|
| **Broker + model sessions (recommended, phase A)** | Any GPU, NPU or hosted API with budgets, handles and audit; per-token overhead target under 5% (`accel.token_rtt_us`) | Vendor-neutral where nvproxy is CUDA-only and E2B has no GPU | Low: no driver, testable in CI with llama.cpp on CPU | Medium |
| **Session fork and KV in cell frames (recommended, phase B)** | Fork of a chat agent no longer re-prefills: about 0.27 s of H100 time saved per copy for an 8k context of an 8B model, tens of seconds on a laptop CPU (derived) | No competitor forks model state; Modal snapshots a function, not a conversation | Medium: engine KV APIs move | Medium |
| **Resume without re-prefill on any matching broker (recommended, phase B)** | A saved agent answers its first token after a 21 ms KV import instead of a full prefill (derived, 1 GiB over PCIe Gen5) | Beyond Modal and Dynamo, which restore a process, not an agent's conversation across machines | Medium | Low once B exists |
| **Hosted APIs with cache-aware routing (recommended, phase C)** | Ten forks write one provider cache entry; reads at 0.1× input price | Per-domain cache keys close a leak found at 7 of 17 providers | Low | Low |
| **Domain-exclusive devices and partitions, scrub (recommended, phase D)** | Safe multi-tenant GPUs | Same as GKE (MIG) plus scrub and per-tree budgets | Medium: needs GPU CI | Medium |
| **Compute sessions by driver-API remoting (phase E)** | Cells run PyTorch and CUDA code | Matches gVisor nvproxy and Modal, adds lazy fork | High: API coverage, vendor checkpoint support | High |
| Passthrough and a driver in Rhizo (option a) | Native speed | Unique, but only on QEMU and Cloud Hypervisor | Very high: licensing, closed user space, firmware | Very high: rejected |
| `ioctl` forwarding like nvproxy | CUDA in cells | Parity with GKE | High: version tables, VM boundary | High: rejected |
| virtio-gpu DRM native context | Vulkan compute on AMD, Intel, Qualcomm in cells | Unique among sandboxes | Medium | High: deferred |
| Confidential GPUs (phase F) | Tenants can verify the GPU and the broker | Few sandboxes offer it | Medium | Medium |

**Recommended architecture:** a host-side accelerator broker (host, or
Linux driver VM on servers) owns every device and vendor stack; the Rhizo
kernel speaks one vendor-neutral protocol to it and exposes it to cells
only through granted handles and channel endpoints; model sessions first,
with engine block sharing for fork and engine-neutral KV kept in the
cell's own frames for suspend, save and move; hosted APIs through the
same broker; hardware partitions between domains; compute sessions later.

## Sources

- gVisor GPU support (nvproxy): https://gvisor.dev/docs/user_guide/gpu/
- GKE Sandbox: https://docs.cloud.google.com/kubernetes-engine/docs/concepts/sandbox-pods
- CRIUgpu: https://arxiv.org/abs/2502.16631 and https://arxiv.org/html/2502.16631v1
- Modal GPU memory snapshots: https://modal.com/blog/gpu-mem-snapshots
- NVIDIA Dynamo Snapshot: https://developer.nvidia.com/blog/nvidia-dynamo-snapshot-fast-startup-for-inference-workloads-on-kubernetes/
- CUDA checkpointing API: https://docs.nvidia.com/cuda/cuda-driver-api/group__CUDA__CHECKPOINT.html
- CUDA virtual memory management: https://docs.nvidia.com/cuda/cuda-driver-api/group__CUDA__VA.html
- LeftoverLocals: https://leftoverlocals.com, https://blog.trailofbits.com/2024/01/16/leftoverlocals-listening-to-llm-responses-through-leaked-gpu-local-memory/, https://arxiv.org/abs/2401.16603
- NVBleed: https://arxiv.org/html/2503.17847v1
- MIG cache attacks: https://www.usenix.org/system/files/conference/usenixsecurity26/sec26_prepub_gu-cheng.pdf
- Virtualized GPU side channels: https://www.ndss-symposium.org/?p=23666
- Cloud Hypervisor VFIO: https://github.com/cloud-hypervisor/cloud-hypervisor/blob/main/docs/vfio.md
- Firecracker PCIe roadmap: https://github.com/firecracker-microvm/firecracker/pull/4894, https://github.com/firecracker-microvm/firecracker/issues/849
- NVIDIA open kernel modules: https://download.nvidia.com/XFree86/Linux-x86_64/595.84/README/kernel_open.html
- amdgpu license: https://lore.kernel.org/all/20221121172058.694436-1-alexander.deucher@amd.com/T/
- Asahi driver: https://patchwork.kernel.org/project/dri-devel/patch/20230307-rust-drm-v1-18-917ff5bc80a8@asahilina.net/mbox
- AMD XDNA driver: https://github.com/amd/xdna-driver
- virtio-gpu DRM native context: https://patchew.org/QEMU/20260306195048.2869788-1-alex.bennee@linaro.org/20260306195048.2869788-13-alex.bennee@linaro.org, https://www.collabora.com/news-and-blog/blog/2025/01/15/the-state-of-gfx-virtualization-using-virglrenderer/
- vAccel and virtio-accel: https://github.com/nubificus/virtio-accel
- API remoting network requirements: https://arxiv.org/abs/2401.13354
- rCUDA over networks: https://riunet.upv.es/bitstream/10251/67332/1/reano_cluster2013_autor.pdf
- NVIDIA MIG profiles: https://docs.nvidia.com/datacenter/tesla/mig-user-guide/620/supported-mig-profiles.html
- MPS, MIG and time-slicing: https://arxiv.org/html/2604.22430v2
- AMD MI300X partitions: https://instinct.docs.amd.com/projects/amdgpu-docs/en/develop/gpu-partitioning/mi300x/overview.html, https://rocm.blogs.amd.com/software-tools-optimization/compute-memory-modes/README.html
- H100 specifications: https://www.nvidia.com/en-us/data-center/h100/, https://developer.nvidia.com/blog/nvidia-hopper-architecture-in-depth/
- H100 confidential computing: https://developer.nvidia.com/blog/confidential-computing-on-h100-gpus-for-secure-and-trustworthy-ai/, https://arxiv.org/html/2409.03992v2
- PagedAttention: https://arxiv.org/abs/2309.06180
- LMCache: https://arxiv.org/abs/2510.09665
- Mooncake: https://www.usenix.org/conference/fast25/presentation/qin
- CachedAttention: https://arxiv.org/abs/2403.19708
- SGLang HiCache: https://lmsys.org/blog/2025-09-10-sglang-hicache
- llama.cpp KV reuse and slots: https://github.com/ggml-org/llama.cpp/discussions/13606
- Prompt caching and sticky routing: https://openrouter.ai/docs/guides/best-practices/prompt-caching, https://openrouter.ai/blog/tutorials/prompt-caching-sticky-routing/
- Auditing prompt caching: https://arxiv.org/abs/2502.07776
- InputSnatch: https://arxiv.org/abs/2411.18191
- vLLM `cache_salt` bypass: https://access.redhat.com/security/cve/CVE-2026-105752
- Modal sandboxes: https://modal.com/resources/best-sandboxes-streaming-code-execution-ai-apps
- E2B GPU FAQ: https://docs.e2b.dev/faq/gpu-support
- RunPod cold starts: https://www.runpod.io/blog/cut-vllm-cold-starts-runpod-serverless, https://www.runpod.io/blog/introducing-flashboot-serverless-cold-start
