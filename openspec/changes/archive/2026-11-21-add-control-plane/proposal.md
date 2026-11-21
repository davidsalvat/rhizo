# Proposal

## Why

Rhizo's cells can be forked, suspended and resumed, but only by code
compiled into the kernel (`Host::*` in `kernel/src/cell.rs`): the in-kernel
tests and the benchmark kernel drive cells themselves, and the live agent
demo of `add-agent-workload` is one more benchmark mode
(`bench=agent-demo`). The vision ("Two clicks", `docs/vision.md`) needs the
opposite split: the host decides (which program, which files, when to
suspend or fork) and the kernel only executes. That needs a control plane:
a channel between the host and a running kernel, a protocol on it, and a
host tool. This change is its first, minimal version ("line G").

## What Changes

- **Control kernel.** A `control` kernel build boots, then serves requests
  instead of running tests or benchmarks. It runs, lists, suspends,
  resumes, forks and kills cells, streams their output, writes a file into
  a running cell, reports memory and cells, and powers off on request.
  Each reply carries the time the kernel took, in nanoseconds. A cell keeps
  its identifier across suspend and resume.
- **Transport: the serial line, on every VMM.** QEMU, Firecracker and Cloud
  Hypervisor all give the guest one UART on their standard input and
  output; Firecracker has no virtio-console and no second UART. The
  control kernel uses that UART both ways (16550 on x86_64 and on
  Firecracker's AArch64, PL011 on QEMU `virt` and Cloud Hypervisor's
  AArch64), its console text and the protocol's frames sharing it.
- **Protocol** (`crates/rhizo-control`, new, `no_std`, no dependency):
  versioned, length-prefixed frames with a request identifier, telling
  frames from console text; requests and replies; checked decoding that
  never panics, fuzzed with proptest.
- **`rhizo`** (`cli/`, new host-only crate `rhizo-cli`, Linux): `up`
  (boots the control kernel on a VMM, leaves a background server that owns
  the VMM and relays a Unix socket to its serial line, and a state file),
  `run <template>`, `ps`, `stats`, `fork`, `suspend`, `resume`, `attach`,
  `kill`, `down`; each prints the kernel's time and the round trip.
  Templates are the programs and file sets the kernel already holds;
  `python-agent` is the CPython agent with its files, and
  `rhizo run python-agent --llm URL --model NAME` reproduces the live
  demo through the control plane (suspend and resume after every turn, a
  fork into three branches at the decision point, model time apart from
  Rhizo's).
- `xtask` becomes a library and a binary, so `rhizo` builds the kernel,
  launches VMMs and sets up the host's network side with the same code.
- `cargo xtask demo agent` becomes a thin wrapper around `rhizo up`,
  `rhizo run python-agent` and `rhizo down`; the benchmark kernel's
  `bench=agent-demo` mode and the agent's live output in the kernel go
  away. The agent waits at its decision point by looking for `/branch`,
  so a plain fork wakes its branches (templates still work).
- Tests: protocol host and property tests, `rhizo` argument parsing, and a
  `control` step in `cargo xtask test` (so in CI on both architectures)
  that boots the control kernel on QEMU with `rhizo up` and drives `run`,
  `suspend`, `resume`, `fork`, `kill`, `ps` and the agent against the fake
  model, checking its transcript.
- Metrics: `control.roundtrip_us` (a `list` from the host through the
  kernel and back) and `control.{run,suspend,resume,fork,kill}_us` (each
  operation on an idle cell as the host sees it), per VMM.

### Out of scope

A graphical app, a template catalog or downloads, an HTTP API,
authentication, several users, snapshots on disk, vsock or virtio-console,
UART interrupts (the kernel polls its UART), macOS and Windows hosts.

## Capabilities

### New Capabilities

- `control-plane`: the control kernel, its protocol and transport, and the
  `rhizo` tool.

### Modified Capabilities

- `benchmarks`: control-plane metrics.

## Impact

New crates `crates/rhizo-control` and `cli` (rows in `xtask/src/deps.rs`
and the AGENTS.md code map). `kernel`: feature `control` (and `cells`, the
features that run cells), `src/control.rs`, the UARTs' receive side behind
`arch::console::control`, `Host::look_every`, `take_output`,
`spawn_with_env`, `has_finished`, `find_image`. `xtask`: `lib.rs` (was
`main.rs`), `control.rs`, `RunOptions::control`, the `control` test step,
the control metrics, `demo.rs` as a wrapper. `cells/python/agent_loop.py`
waits for `/branch`. `docs/demo.md`, README, `bench/README.md`. No new
external dependency. No benchmarked path changes: the control kernel is a
build of its own, and the hooks it adds to the host (`look_every`) are off
elsewhere.
