# Proposal

## Why

The thesis compares cells with "microVM- and userspace-kernel-based
sandboxes". Firecracker (microVM) is measured; gVisor, the main
userspace-kernel sandbox, is not. Without it the claim is half-checked.

## What Changes

- `cargo xtask bench` measures gVisor (`runsc`) on the same machine, in the
  same run, where `runsc` is installed and runs:
  - `sandbox.run_ms` for guest `gvisor`: a new sandbox running the same
    static `hello` the cell benchmark runs, to its exit (`runsc run`, KVM
    platform);
  - `snapshot.restore_ms` for guest `gvisor`: restoring a checkpointed
    sandbox of an idle program until it runs again (`runsc checkpoint` /
    `runsc restore`).
- CI installs `runsc` on the benchmark job: the Ubuntu package (signed
  archive), or a fixed gVisor release verified against its published
  SHA-512.
- Where `runsc` is missing or cannot run, the comparison is skipped with
  the reason, like Firecracker.

### Out of scope

gVisor platforms other than KVM, network setups, OCI runtimes
around it (containerd, Docker).

## Capabilities

### Modified Capabilities

- `comparisons`: gVisor.

## Impact

`xtask/src/compare.rs`, `xtask/src/bench.rs`, `.github/workflows/ci.yml`,
`bench/README.md`, README.
