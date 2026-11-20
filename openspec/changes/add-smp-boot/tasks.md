# Tasks

## 1. Library code

- [x] 1.1 `rhizo_core::options`: `smp=N`; host tests; verify `cargo test -p rhizo-core options`
- [x] 1.2 `rhizo_core::fdt::cpus` (CPU nodes, two-cell identifiers, status, PSCI), QEMU `-smp 4` fixture; verify `cargo test -p rhizo-core fdt`
- [x] 1.3 `rhizo_core::acpi`: RSDP (hint, EBDA, BIOS area), RSDT/XSDT, MADT processors, checksums; host tests; verify `cargo test -p rhizo-core acpi`
- [x] 1.4 `rhizo_core::smp::Cpus::select` (D2) with a property test; `rhizo_core::sync::SpinLock` (D7) with threads as CPUs; verify `cargo test -p rhizo-core`

## 2. Kernel

- [x] 2.1 `sync::Global` on `SpinLock`, CPU number from `TPIDR_EL1` / GS (D5, D7); `SWAPGS` around cell code; per-CPU GDT and TSS on x86_64; verify `cargo xtask test` on both architectures
- [x] 2.2 `smp`: stacks with guard pages (D4), mailbox, idle and wake-up (D6); `cell::init` (D1); verify `cargo xtask test --only ktest-smp` on both architectures
- [x] 2.3 AArch64 bring-up: PSCI `CPU_ON`, secondary entry, redistributors by affinity, SGI wake-up (D3, D6); verify `cargo xtask test --only ktest-smp`
- [x] 2.4 x86_64 bring-up: MADT, trampoline, INIT/STARTUP, local APIC per CPU, wake-up vector (D3, D6); verify `cargo xtask --arch x86_64 test --only ktest-smp` on microvm and q35
- [x] 2.5 In-kernel tests: every CPU's number and stack, globals and heap from all CPUs at once; verify `cargo xtask test --only ktest-smp` on both architectures

## 3. Tooling and documentation

- [ ] 3.1 `xtask`: `RunOptions::cpus` for every VMM, `ktest-smp` step; CI runs it on Firecracker and Cloud Hypervisor; verify `cargo xtask check` and CI
- [ ] 3.2 `AGENTS.md` (code map, memory layout, arch interface); `cargo xtask ci` on both architectures; CI green
