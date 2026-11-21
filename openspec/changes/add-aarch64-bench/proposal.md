# Proposal

## Why

AArch64 is the primary architecture, yet no benchmark protects it.
`cargo xtask bench` runs with KVM only in the x86_64 `bench` job; the
AArch64 job (`ubuntu-24.04-arm`) has no `/dev/kvm` and runs functional
tests under emulation. The `dedicated` environment for a self-hosted ARM
runner exists, but no such machine is available and none will be added
for now. A change that doubled the cost of a fork on AArch64 alone would
pass CI.

A probe on every GitHub-hosted ARM runner a public repository can use
for free (design D1) found no hardware acceleration: the Linux runners
boot without EL2 (`kvm [1]: HYP mode not available`), the macOS runners
refuse Hypervisor.framework (`HV_UNSUPPORTED` from QEMU's `hvf`), and the
Windows runner is itself a virtual machine without nested virtualization.
Timings under emulation measure QEMU, not Rhizo. Instruction counts under
QEMU's `-icount` do not: they are exact and the same on every run and
host (spec `profiling`, change `add-cost-accounting`), and the x86_64 job
already gates them.

## What Changes

- `cargo xtask bench --icount-only` measures only the instruction counts
  under `-icount` (the `bench-cell` and `profile` kernels once each).
- The AArch64 CI job runs it after its checks in a new `ci-arm`
  environment, gated with `thresholds_pct.instructions` (2 %), breakdowns
  and lock waits ungated as in `ci`, and publishes the results as
  `bench-aarch64-results-N` annotations (medians as
  `bench-aarch64-medians`).
- Under `-icount`, figures the PMU passes leave out (the fork benchmarks)
  come from the clock, as on x86_64, so AArch64 counts the same
  operations x86_64 does.
- `cargo xtask bench --from` can be repeated to merge the AArch64 results
  with the x86_64 ones; `--readme` then adds a table of instructions per
  operation on both architectures. `docs/img/gen.py` takes the AArch64
  results as an optional second file and draws `arches.svg` from them.
- `bench/README.md` records the probe and its sources.

### Out of scope

AArch64 timings in CI: they need KVM on ARM, which GitHub does not offer
to public repositories (larger ARM runners are billed and run on the same
Azure Cobalt 100 virtual machines, which have no nested virtualization).
The `dedicated` workflow stays the way to get them.

## Capabilities

### Modified Capabilities

- `benchmarks`: an AArch64 regression gate on instruction counts, and the
  README pipeline taking both architectures' results.

## Impact

- `xtask/src/bench.rs`, `xtask/src/profile.rs`, `xtask/src/lib.rs`
  (help), `.github/workflows/ci.yml` (AArch64 job), `bench/baseline.json`
  (environment `ci-arm`), `bench/README.md`, `docs/img/gen.py`.
- No kernel code. The AArch64 job gets about a minute longer (two release
  kernels built and run once under TCG).
