# Proposal

## Why

The proof of concept measures each primitive alone (spawn, fork, suspend,
a round trip), never the workloads it is meant for (`docs/vision.md`): a
laptop where a few agents talk to a local model and sit idle most of the
time, and a server packed with thousands of active cells. Bottlenecks that
only show at that scale or in that mix (idle wake-ups, memory per
suspended agent, scheduling tails with 10,000 cells, what happens when
memory runs out) have no metric, so nothing protects them and nothing says
which to fix first. This change adds two reference workloads (F.2) and
absorbs line B.3, density with active cells. The third, a template
downloaded from disk, needs snapshots on disk and comes later.

## What Changes

- **Laptop workload** (`cargo xtask bench --workload laptop`): an `agent`
  cell frozen as a template after its warm-up, 50 agents opened from it,
  45 of them suspended; a fake model server on the host (HTTP, configurable
  delay) reached through the network card; a shared folder seeded into the
  template's files at boot (there is no virtio-fs or vsock yet). Metrics:
  time to open, suspend and resume an agent, one turn (user message,
  request to the model, answer, tool action), total memory, and, while
  every agent waits, the guest's wake-ups per second and the VMM's CPU
  time on the host.
- **Server workload** (`--workload server`): 10,000 cells (or what memory
  allows, with the reason) forked from `worker` templates that compute,
  write and read files, and talk in pairs over loopback TCP, one operation
  per period, while the host suspends, resumes and forks workers at a
  constant rate. Metrics: `density.cells`, throughput, memory per cell,
  p50/p99/p99.9 of scheduling, wake-up and system-call latencies.
- **Out of memory**: after the load, idle cells are forked until the
  kernel refuses one; spawn, fork and eager resume must be refused, `mmap`
  must fail with `ENOMEM`, nothing may panic and all memory must come back.
  A run where this does not hold fails.
- **Instrumentation** for these figures only: latency histograms and a few
  counters (idle wake-ups, socket wake-ups, faults, time in scheduling),
  off unless a workload turns them on; F.1's permanent breakdown replaces
  them when it lands.
- The host can end a sleeping cell's sleep (`Host::wake`), standing in for
  a user's message.
- Both workloads run in `cargo xtask bench` (so in the CI benchmark job);
  a weekly job runs the long versions (`--soak`). Metrics that prove stable
  on CI enter the `ci` baseline; metrics where more is better are gated in
  that direction.

### Out of scope

Snapshots on disk and downloaded templates (the third workload), vsock and
shared folders with the host, the server workload on several CPUs (host
operations stop every CPU, design D2), fixing the bottlenecks found: each
becomes its own change.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `benchmarks`: the laptop and server workloads, their metrics, clean
  out-of-memory failures, the weekly soak, metrics where more is better.
- `cells`: the host wakes a sleeping cell.

## Impact

`cells/src/bin/{agent,worker}.rs` (new), `cells/src/bin/sleeper.rs`
(optional duration), `crates/rhizo-core` (`stats::Histogram`, boot options
`bench=`, `cells=`, `secs=`, `llm=`), `crates/rhizo-sched` (`wake_due`),
`kernel/src/{prof,workloads}.rs` (new), `kernel/src/cell.rs` (hooks,
`Host::wake`, `Host::take_finished`), `kernel/src/tests.rs`,
`xtask/src/{workloads,bench,vmm,hostnet}.rs`, `.github/workflows/weekly.yml`,
`bench/README.md`, `bench/baseline.json`.
