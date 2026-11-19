# Proposal

## Why

The thesis is a comparison: cells create, suspend, resume and fork agent
environments orders of magnitude faster than today's microVM and
userspace-kernel sandboxes. Until now the README compared Rhizo's own boot
with a Linux guest and quoted other projects' published figures, measured on
other hardware. H7 measures the existing mechanisms on the same machine, in
the same CI run, with the same timing method, and publishes the comparison
from those numbers only.

## What Changes

- `xtask bench` measures, next to the cell metrics:
  - Firecracker snapshot restore of a Linux guest (`snapshot.restore_ms`:
    from launching a new Firecracker process to the restored VM running;
    `snapshot.restore_api_ms`: the load request alone). This is how
    Firecracker-based sandboxes resume, and how they clone.
  - Linux processes on the host (`process.fork_us` with 50 MiB written,
    `process.spawn_us` of the same static `hello` program the cells run).
- A generated comparison table in the README ("Cells against existing
  mechanisms") built only from one run's results.
- CI publishes the full results JSON in annotation-sized pieces, so the
  README can be regenerated exactly from a CI run.
- The dedicated-machine benchmark job (tight thresholds) is documented as
  the place for publishable numbers; GitHub's runners are shared and use
  nested virtualization.

### Out of scope

gVisor, E2B, Daytona and other services (not runnable as a local
component in CI), multi-vCPU guests, diff snapshots.

## Capabilities

### New Capabilities

- `comparisons`: measuring existing mechanisms with the same method.

### Modified Capabilities

- `benchmarks`: comparison metrics and the README table.
