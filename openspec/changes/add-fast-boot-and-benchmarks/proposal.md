# Proposal

## Why

Speed is Rhizo's headline claim, so it has to be measured from now on, not
only at milestone H7. Today the kernel boots only on QEMU's `q35` machine
(firmware included), prints through a UART one polled byte at a time, is
built and timed in debug mode, and reports a single in-guest cycle count.
None of that says how long a user actually waits, and nothing stops a pull
request from making things slower.

This change sits between H1 (memory) and H2 (cells). From here on, every
design decision prefers the fastest option that keeps correctness and tests.

## What Changes

- QEMU `microvm` becomes the default machine (with `-accel kvm` when
  available); `q35` stays available as an alternative.
- A fast boot console: QEMU/Cloud Hypervisor debug port `0xE9` by default,
  with the UART kept as fallback (Firecracker has no debug port).
- Host-side measurement: `xtask` reports the time from launching the VMM to
  the kernel being ready, next to the in-kernel cycle counts.
- The same kernel boots on Firecracker and Cloud Hypervisor (PVH), and
  `cargo xtask bench` compares boot time across all supported VMMs.
- Optimized release profile (LTO, `codegen-units = 1`, `opt-level = 3`,
  `panic = "abort"`); benchmarks always use release builds.
- Benchmarks with regression thresholds against a baseline stored in the
  repository: tight thresholds for deterministic metrics, wide thresholds for
  timings on noisy CI runners, precise thresholds on a dedicated KVM machine.
- Direct map built with 1 GiB pages when the CPU supports them (2 MiB
  otherwise), measured by a benchmark.
- README: a generated comparison chart against existing sandboxes, numeric
  targets (hypotheses) against Firecracker, Unikraft and Hyperlight, and the
  roadmap of cell-primitive optimizations, each tied to a benchmark.
- AGENTS.md: every optimization must ship a benchmark that proves it, and no
  pull request may make a metric worse without a written justification.

### Out of scope

Cell primitives themselves (H2+); the optimizations listed for later
milestones are only planned here. ARM, real hardware, networking.

## Capabilities

### New Capabilities

- `boot-performance`: supported VMMs and machine types, the fast console,
  the release profile, and how boot time is measured end to end.
- `benchmarks`: the benchmark command, its metrics, the stored baseline and
  the regression gates in CI.

### Modified Capabilities

- `kernel-boot`: boot output goes to the boot console (fast debug port by
  default, serial fallback) instead of always the serial port; the boot
  report gains a machine-readable "ready" line.

## Impact

- Kernel: console selection, ready marker, 1 GiB direct-map pages, VMM-aware
  exit paths.
- `xtask`: VMM abstraction (QEMU microvm/q35, Firecracker, Cloud
  Hypervisor), `bench` command, baseline comparison, chart generation.
- New files: `bench/baseline.json`, `bench/README.md`.
- CI: a benchmark job with wide thresholds; an optional precise job for a
  self-hosted KVM runner.
- New tooling downloaded by `xtask` (not linked): Firecracker and Cloud
  Hypervisor release binaries (Apache-2.0), pinned by version and checksum.
