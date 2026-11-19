# Proposal

## Why

Rhizo's primitives (cell switch, suspend/resume, fork) are dominated by
address-space and TLB costs, and AArch64 is better suited to them than x86_64:

- **16-bit ASIDs** (65,536 address-space tags, versus 4,096 PCIDs on x86_64)
  let thousands of cells keep their TLB entries across switches.
- **Broadcast TLB invalidation in hardware** (`TLBI ... IS`) invalidates a
  page on every core without inter-processor interrupts, which x86_64 needs
  (TLB shootdowns) for copy-on-write and unmapping on multiple cores.
- **Lighter virtualization:** a dedicated hypervisor level (EL2, with VHE)
  and simple boot (Image header + device tree, no firmware, no legacy
  devices) give smaller VMMs and faster guest entry.
- **Server efficiency:** Graviton, Ampere and Cobalt instances offer more
  cores per watt and per dollar, which matters for dense, mostly idle cells.

H2–H5 depend heavily on the architecture (user mode, syscalls, address
spaces, copy-on-write), so the switch happens now, before H2.

## What Changes

- AArch64 becomes the primary architecture: Linux arm64 Image boot protocol
  with a device tree, entering EL1 (dropping from EL2 when entered there).
- Memory, console (PL011 or 16550, whichever the device tree describes) and
  PSCI found through a new, host-tested device-tree parser.
- New `kernel/src/arch/aarch64`: MMU (TTBR0/TTBR1, direct map, higher-half
  kernel, W^X, block mappings), EL1 exception vectors with fault reports
  equivalent to today's, a separate exception stack so stack overflows are
  reported, generic-timer cycle counter (`CNTVCT_EL0`/`CNTFRQ_EL0`) which
  also gives boot time in nanoseconds, PSCI power-off.
- `rhizo-mm` page tables become generic over the entry format, with an
  x86_64 and an AArch64 encoding.
- `xtask` gains `--arch`; QEMU `virt` (KVM when available), Firecracker and
  Cloud Hypervisor on AArch64; reference Linux guest for arm64.
- CI runs the AArch64 suite on GitHub's ARM64 runners (emulated: they do not
  expose KVM) and keeps the x86_64 suite and benchmark gate on x86_64 runners
  (which do expose KVM).
- Documentation: why ARM64, how to measure on real ARM64 hardware with KVM
  (`dedicated` environment), updated README, AGENTS.md, CONTRIBUTING.md.

### Out of scope

Multicore, interrupts (GIC), user mode and cells (H2), UEFI/ACPI boot,
big-endian, 16K/64K granules, real-hardware boards without a hypervisor.

## Capabilities

### New Capabilities

- `device-tree`: reading memory, devices, PSCI and the command line from a
  flattened device tree.
- `architectures`: which architectures are supported, what each must
  implement behind `arch/`, and how they are tested.

### Modified Capabilities

- `kernel-boot`: boot protocols per architecture (PVH on x86_64, arm64 Image
  + device tree on AArch64) and their consoles and exits.
- `boot-performance`: supported VMMs per architecture; boot time reported in
  nanoseconds where the architecture provides a timer frequency.
- `cpu-exceptions`: architecture-neutral wording (fault address and access
  kind, separate stack for stack overflow) with per-architecture reports.

(`virtual-memory` keeps its requirements: they hold unchanged on both
architectures; only the page-table entry encoding behind them differs.)

## Impact

- New target `aarch64-unknown-none-softfloat` in `rust-toolchain.toml`.
- New code: `kernel/src/arch/aarch64/`, device-tree parser (in
  `rhizo-core`), AArch64 page-table format (in `rhizo-mm`), `xtask --arch`.
- CI: AArch64 job on `ubuntu-24.04-arm` (emulated), x86_64 jobs unchanged.
- No new dependencies (the common `fdt` crate is MPL-2.0, outside the license
  allow-list; a minimal parser is written instead).
