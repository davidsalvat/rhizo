# Design

## Context

The repository is empty apart from documentation. See `proposal.md` for the
motivation and `specs/` for the required behavior. Two constraints from
`AGENTS.md` shape this design more than anything else:

- Kernel core logic must reach hardware only through narrow interfaces, so it
  can later run inside a deterministic simulator on the host.
- Contributors (human or AI) must be able to build and test with nothing but
  the Rust toolchain and QEMU.

## Goals / Non-Goals

**Goals:**

- Fastest possible path from "VM starts" to "kernel code runs", because boot
  and resume latency are the project's headline metrics.
- A test harness that is usable from day one and that CI can trust.
- A code layout that keeps hardware-facing code small and isolated.

**Non-Goals:**

- Higher-half kernel mapping, physical memory management, heap (H1).
- Interrupt handling (IDT) beyond what is needed to not crash. Exceptions in
  H0 end in a triple fault, which QEMU reports as a non-success exit.
- Calibrating the CPU clock frequency (boot time is reported in TSC cycles).

## Decisions

### D1. Boot protocol: PVH direct boot

The kernel is an ELF image carrying a PVH entry note
(`XEN_ELFNOTE_PHYS32_ENTRY`). QEMU boots it with `-kernel`; Firecracker and
Cloud Hypervisor, the microVM monitors we will compare against and may run on,
also boot PVH kernels. The hypervisor enters the kernel in 32-bit protected mode
with paging off and a pointer to a `hvm_start_info` structure in `ebx`. A small
assembly trampoline validates the start-info magic, builds identity-mapped page
tables for the first 1 GiB (2 MiB pages), enables long mode and jumps to Rust.

Alternatives considered:

- **Limine bootloader.** Popular for hobby kernels and hands us a 64-bit
  environment, but needs an ISO image, `xorriso` and Limine binaries, and goes
  through firmware, which adds boot latency. Rejected: extra tools and slower.
- **`bootloader` crate (rust-osdev).** Pure-Rust image builder, but builds
  BIOS/UEFI disk images and boots through firmware. Rejected: slower boot and
  a design centered on firmware we do not want.
- **Multiboot / Multiboot2.** QEMU's `-kernel` only supports Multiboot 1 for
  32-bit images; Multiboot2 needs GRUB and an ISO. Rejected.

### D2. Toolchain: stable Rust, `x86_64-unknown-none`

The toolchain is pinned in `rust-toolchain.toml` to a stable release with the
`x86_64-unknown-none` target, `rustfmt` and `clippy`. This target ships
prebuilt `core` and `alloc` on stable, so no `build-std` and no nightly.

Alternative: nightly with `custom_test_frameworks`, the classic way to run
`cargo test` inside a kernel. Rejected: that feature has been unstable for
years and a nightly pin makes contributor setup and CI more fragile. We write
our own minimal test registry instead (D5).

### D3. Workspace layout

```
kernel/              Bootable kernel (bin crate, x86_64-unknown-none only)
  src/arch/x86_64/   PVH entry trampoline, serial port, QEMU exit, TSC
crates/rhizo-hal/    Hardware interfaces (traits): console, power, cycle counter
crates/rhizo-core/   Hardware-independent kernel logic (no_std, host-testable)
xtask/               Host developer tool: build, run, test (runs QEMU)
```

`rhizo-core` and `rhizo-hal` are `no_std` libraries that also compile for the
host, so `cargo test` runs their tests natively with test doubles. Only
`kernel/` contains `unsafe` hardware access and assembly.

Alternative: a single kernel crate with `cfg(test)` tricks. Rejected: the
host/target boundary becomes invisible and erodes over time.

### D4. Hardware interfaces in H0

Three narrow traits in `rhizo-hal`:

- `Console`: write bytes.
- `Power`: exit with success or failure.
- `CycleCounter`: read a monotonically increasing cycle count.

Core code (banner, boot report, test runner) is written against these traits.
The x86_64 implementations are a minimal 16550 UART driver on COM1 (`0x3F8`),
QEMU's `isa-debug-exit` device (`0xF4`) and `rdtsc`. All three are a few lines
of port I/O, so we write them rather than add crates.

### D5. In-kernel test harness

The kernel has a `ktest` Cargo feature. With it enabled, after boot the kernel
runs a static list of `(name, fn)` test cases instead of the normal shutdown
path. The runner logic (iteration, output format, final status) lives in
`rhizo-core` and is itself host-tested against a fake console.

A failing test panics; the panic handler prints the test name and message and
exits with failure. H0 therefore stops at the first failing test, which is
acceptable at this size. Per-test isolation (one VM per test, or catching
failures) can come later without changing the spec.

Alternative: linker-section registration (distributed slices) so tests
self-register. Deferred: nice to have, not needed for a handful of tests.

### D6. Exit status mapping

QEMU's `isa-debug-exit` turns a written value `v` into process exit status
`(v << 1) | 1`. The kernel writes `0x10` for success (status 33) and `0x11`
for failure (status 35). `xtask` maps 33 to 0 and anything else (35, a QEMU
error, a triple-fault reset with `-no-reboot`, a timeout) to non-zero.

### D7. Developer commands (`cargo xtask`)

The `xtask` pattern keeps all tooling in Rust, with no Makefile or shell
scripts:

- `cargo xtask build`: build the kernel image.
- `cargo xtask run`: build and boot it in QEMU with the serial port on stdout.
- `cargo xtask test`: host tests, a boot check of the normal kernel, the
  in-kernel `ktest` build in QEMU, and two harness self-checks (a suite with a
  panicking test must fail with a panic report; a hanging suite must time
  out). Timeout per VM: default 30 s, overridable with `RHIZO_TEST_TIMEOUT`.
- `cargo xtask ci`: `cargo fmt --check`, clippy with warnings denied (host
  crates, and the kernel with every feature set), then `cargo xtask test`.

QEMU runs with `-machine q35 -cpu max -display none -serial stdio -no-reboot`.
`xtask` passes `-accel kvm` when `/dev/kvm` is usable and `-accel tcg`
otherwise, so runs never print KVM fallback noise. QEMU's `microvm` machine
type will be evaluated later for boot-latency work.

### D8. Continuous integration

GitHub Actions on Ubuntu 24.04: install `qemu-system-x86`, enable KVM when the
runner exposes it, install the pinned toolchain, then run `cargo xtask ci`, so
CI and contributors run exactly the same checks.

### New external dependencies

None in the kernel or core crates. Tooling: QEMU (GPL-2.0, used as an external
program and not linked or vendored, so it has no effect on Rhizo's license).

## Risks / Trade-offs

- [Bugs in the 32→64-bit trampoline are hard to debug] → Keep it minimal,
  document every step, and use QEMU's `-d int,cpu_reset` and the gdb stub
  (`cargo xtask run --gdb`) when needed.
- [CI runners have no KVM, so QEMU is emulated and slow] → H0 makes no
  performance assertions; boot time is only reported. Performance gates arrive
  with dedicated benchmark infrastructure (H7).
- [Stopping at the first failing in-kernel test hides later failures] →
  Acceptable for H0's handful of tests; revisit when the suite grows.
- [Boot time in TSC cycles is not comparable across machines] → Accepted for
  H0; a calibrated time unit comes with timer work.

## Open Questions

- Minimum supported QEMU version. PVH boot needs QEMU 4.0 or later; we will
  record the oldest version CI has verified.
