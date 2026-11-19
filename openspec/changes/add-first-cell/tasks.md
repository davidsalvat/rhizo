# Tasks

## 1. Shared crates

- [x] 1.1 Create `rhizo-cell` with the `Cell`/`Registers` model and the static ELF loader (validation, segment plan, W^X, stack with argv/envp/auxv); add it to the dependency table; verify host tests with hand-built ELFs and the test programs
- [x] 1.2 Create `rhizo-abi` with dispatch tables for both architectures and the H2 calls, `EFAULT`, `ENOSYS` reporting; verify host tests against a fake cell context
- [x] 1.3 Cell address spaces in `rhizo-mm` terms: per-cell root sharing the kernel half, user mappings, 2 MiB anonymous mappings, teardown returning every frame; verify host tests

## 2. Architectures

- [x] 2.1 AArch64: EL0 entry/return and `SVC` path with `SP_EL1` as entry stack (design D3, option B), user faults routed to the cell; verify `hello` and the cell self-checks under QEMU `virt`
- [x] 2.2 x86_64: user segments, `SYSCALL`/`SYSRET` (IRETQ fallback), TSS `RSP0`, `arch_prctl`; verify `hello` and the cell self-checks under QEMU `microvm`

## 3. Programs, tests and benchmarks

- [x] 3.1 `cells/` test programs (hello, exit status, brk/mmap, bad pointer, unknown syscall, fault) built by `xtask`; differential run on a Linux host of the same architecture when available
- [x] 3.2 In-kernel tests spawning each program; self-check: a faulting cell is killed and the kernel continues
- [x] 3.3 `bench-cell` feature and `cell.spawn_us`, `cell.pt_frames` metrics in `xtask bench`

## 4. Documentation

- [x] 4.1 README roadmap (H2), AGENTS.md code map (new crates, cell layout), `cargo xtask ci` on both architectures

## Workflow follow-up

- Archive this change after merge.
