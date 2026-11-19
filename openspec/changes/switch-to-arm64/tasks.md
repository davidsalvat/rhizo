# Tasks

## 1. Shared code

- [x] 1.1 Make `rhizo-mm` page tables generic over `EntryFormat` with `X86_64` and `Aarch64` encodings; verify every behavior test runs for both formats and encoding tests check AArch64 descriptors bit by bit
- [x] 1.2 Add the device-tree reader to `rhizo-core`; verify host tests with QEMU's `virt` device tree (fixture) and a hand-built tree with 1-cell and 2-cell nodes and two memory banks
- [x] 1.3 Add AArch64 exception reports and the `exit=psci` / `exit=semihosting` options to `rhizo-core`; verify host tests decode data aborts, instruction aborts, undefined instructions and stack overflows
- [x] 1.4 Move every x86 assumption out of shared kernel code behind the documented `arch` interface (`boot`, `platform`, `console`, `power`, `cpu`, `timer`, `tests`); verify `cargo xtask --arch x86_64 test` still passes

## 2. AArch64 kernel

- [x] 2.1 Entry code: Image header, EL2 → EL1, `.bss` zeroing and cache invalidation, stage-1 tables, MMU on, higher-half jump, `SP_EL0`/`SP_EL1` stacks, vectors; verify the kernel prints its banner on QEMU `virt`
- [x] 2.2 Platform from the device tree (RAM, reserved device tree, command line, UART), PL011 and 16550 consoles, PSCI and semihosting exit, generic-timer counter; verify `cargo xtask test --only boot`
- [x] 2.3 Exception vectors and reports, broadcast TLB invalidation, TTBR0/TTBR1 activation; verify every self-check passes on AArch64, including the stack-overflow report
- [x] 2.4 Architecture tests (`device_tree_is_valid`, `counter_frequency_is_known`) and an architecture-neutral direct-map test; verify `cargo xtask test --only ktest` (18 tests)

## 3. Tooling and CI

- [x] 3.1 `xtask --arch` (default `aarch64`), ELF to flat Image, QEMU `virt`, per-architecture VMM command lines, self-check expectations and tool downloads; refuse unpinned downloads unless allowed; verify `cargo xtask test` for both architectures
- [x] 3.2 arm64 reference guest (hand-assembled `/init`, Firecracker's arm64 CI kernel, measured on Firecracker only); verify unit tests decode the `adr`/`mov` encodings
- [ ] 3.3 CI: AArch64 job on `ubuntu-24.04-arm` (emulated, reports whether `/dev/kvm` exists), x86_64 jobs run with `--arch x86_64`; verify both pass on the pull request

## 4. Documentation

- [ ] 4.1 README: why ARM64, architecture decision, getting started on both architectures, roadmap; AGENTS.md: commands, `arch` interface, layout; CONTRIBUTING.md: prerequisites; bench/README.md: ARM64 dedicated hardware and known issues; verify `cargo xtask check`

## Workflow follow-up

- Archive this change with `openspec archive switch-to-arm64` after it is merged.
- Pin the AArch64 Firecracker and Cloud Hypervisor checksums on the first run on ARM64 hardware with KVM, and verify both boots there.
