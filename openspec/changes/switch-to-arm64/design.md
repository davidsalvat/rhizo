# Design

## Context

See `proposal.md`. Today everything architecture-specific lives in
`kernel/src/arch/x86_64` except two things that must move behind the
boundary: the PVH memory map (`rhizo-mm::memmap`, read by `kernel/src/memory.rs`)
and the x86_64 page-table entry encoding (`rhizo-mm::paging`). Specs describe
behavior; a few scenarios name x86 details (PVH, vectors) and are reworded.

## Goals / Non-Goals

**Goals:** AArch64 reaches parity with every current spec on QEMU `virt`;
shared code stays architecture-neutral; x86_64 keeps passing (see D1).

**Non-Goals:** GIC and interrupts, multicore, user mode (H2), ACPI/UEFI boot.

## Decisions

### D1. What happens to x86_64

| Option | Cost | For | Against |
|---|---|---|---|
| A. Remove x86_64 | None ongoing; −1.5k lines | Minimal code; one architecture to reason about | Loses the only place where CI can measure with KVM today (GitHub's x86_64 runners expose KVM; ARM64 runners do not); competitors (Agent Substrate, Firecracker, E2B) run mostly on x86_64, so comparisons on x86_64 are what adopters ask for; work already done and benchmarked is thrown away |
| B. Keep x86_64 as a second architecture behind `arch/`, same specs, same tests (recommended) | Each architecture hook is written twice (boot, MMU entry format, exceptions, console, power; later: user-mode entry and syscalls). Estimated +25–35% of architecture-specific work per milestone, ~0% for everything in `rhizo-*` crates | CI keeps a KVM benchmark gate; comparisons on the platform rivals use; two implementations keep the `arch/` boundary honest (nothing x86-specific leaks into shared code) | More code to maintain; every arch hook needs two reviews |
| C. Freeze x86_64 (kept in tree, not built or tested) | None now | Easy to revive | Untested code rots; violates "delete dead code" |

**Recommendation: B, with AArch64 first.** Specs stay architecture-neutral
and every spec scenario runs on both architectures in CI. New hooks are
written for AArch64 first and must reach x86_64 within the same milestone
(the milestone is not done until both pass). The minimal-code rule is
respected because the shared logic is written once (`rhizo-*` crates), and
only the thin `arch/` layer is duplicated. If, at H7, x86_64 has no users and
ARM64 hardware runs the benchmark gate, option A becomes a one-PR deletion.

### D2. Boot protocol and entry

- The image starts with the 64-byte Linux arm64 header (branch, `text_offset`
  0, image size, flags: little endian, 4 KiB pages, placeable anywhere
  2 MiB-aligned, magic `ARM\x64`). `xtask` turns the ELF into a flat Image.
- On entry: `x0` = physical address of the device tree, MMU and caches off.
  If `CurrentEL` is EL2, the entry code sets `HCR_EL2.RW`, allows EL1 timer
  access, and `eret`s to EL1; other ELs halt.
- The image is linked at a fixed higher-half address and runs from wherever
  it was loaded: early code only uses PC-relative addressing (`adrp`/`add`),
  computes the physical load address, and builds stage-1 tables that map the
  image both at its physical address (identity, for the MMU switch) and at
  the link address.

### D3. Device tree

A minimal flattened-device-tree reader in `rhizo-core` (no allocation):
header and structure-block walk, `#address-cells`/`#size-cells`, and lookups
for `/memory` `reg`, `/chosen` `bootargs`, `/psci` `method`, the first UART
(`arm,pl011` or `ns16550a`) and `/reserved-memory`. Host tests use device
trees produced by QEMU `virt` and by Firecracker, committed as fixtures. The
same command-line options as today (`console=`, `exit=`) are read from
`bootargs`.

Alternative: the `fdt` crate. Rejected: MPL-2.0 is outside the license
allow-list.

### D4. MMU and layout (AArch64)

4 KiB granule, 48-bit virtual addresses, four levels. `TTBR1_EL1` holds the
kernel half (`0xffff_…`), `TTBR0_EL1` stays empty for cells (H2). Layout
matches x86_64: direct map at `0xffff_8000_0000_0000 + phys`, kernel at
`0xffff_ffff_8000_0000 + offset`. Memory attributes via `MAIR_EL1`: normal
write-back (index 0) for RAM, device nGnRE (index 1) for the UART. W^X with
`AP`/`UXN`/`PXN`; direct map with 1 GiB blocks where aligned, 2 MiB
otherwise. `rhizo-mm::paging` becomes `PageTable<F: EntryFormat>`; the walk
is shared and `X86_64` / `Aarch64` encode entries. TLB maintenance: `TLBI
VAE1IS` + `DSB ISH` + `ISB`.

### D5. Exceptions

`VBAR_EL1` points to a 2 KiB-aligned table of 16 vectors. The kernel runs on
`SP_EL0` (`SPSel = 0`); exceptions taken at EL1 switch to `SP_EL1`, a
dedicated exception stack, so a kernel stack overflow into its guard page is
still reported (the AArch64 equivalent of the x86_64 IST double-fault stack).
Reports decode `ESR_EL1` (exception class, data/instruction abort, read or
write, translation or permission fault) and `FAR_EL1`, and are formatted in
`rhizo-core` (host-tested) like today's.

### D6. Console, exit, timer

- Console: PL011 (QEMU `virt`, Cloud Hypervisor) or 16550 MMIO (Firecracker),
  from the device tree; polled transmit.
- Exit: PSCI `SYSTEM_OFF` through the method in the device tree. Under QEMU
  with emulation, the semihosting `SYS_EXIT_EXTENDED` call gives `xtask` a
  success/failure exit status (enabled with `-semihosting`); elsewhere the
  verdict comes from the console, as on Firecracker today.
- Cycle counter: `CNTVCT_EL0`; `CNTFRQ_EL0` converts it to nanoseconds, so
  boot time is reported in µs on AArch64.

### D7. VMMs, xtask and CI

- `cargo xtask <cmd> --arch aarch64|x86_64` (default `aarch64`); VMMs per
  architecture: AArch64: `qemu-virt` (default), `firecracker`,
  `cloud-hypervisor`; x86_64: unchanged.
- Reference guest on arm64: Firecracker's CI arm64 kernel with an arm64
  hand-assembled `/init`.
- CI: `ubuntu-24.04-arm` runs `cargo xtask ci --arch aarch64` under emulation
  (no KVM there; a job step prints whether `/dev/kvm` exists, to detect if
  GitHub ever enables it). x86_64 jobs unchanged, including the KVM benchmark
  gate. AArch64 benchmarks start in the `dedicated` environment.

### D8. Real hardware (`dedicated` environment)

Any ARM64 Linux host with KVM: Raspberry Pi 5 (Cortex-A76, 8 GB), Orange Pi 5
(RK3588), or cloud bare metal / nested-virtualization instances (AWS Graviton
`*.metal`, Ampere Altra). Steps (documented in `bench/README.md`): install
QEMU and rustup, check `/dev/kvm`, register a self-hosted runner labelled
`rhizo-bench` and `arm64`, run the `bench-dedicated` workflow.

## Risks / Trade-offs

- [Firecracker and Cloud Hypervisor on AArch64 cannot be tested in CI (no
  KVM)] → their launch code is unit-tested; boots are verified on the
  dedicated machine and marked "unverified in CI" until then.
- [Two architectures to maintain] → see D1; shared logic in `rhizo-*`.
- [Emulated AArch64 timings are meaningless] → not gated; reported only.
