# Tasks

## 1. Scheduling core

- [x] 1.1 Create `rhizo-sched` (run queue, sleep list, round robin, explicit time) with host tests; add it to the dependency table
- [x] 1.2 `rhizo-abi`: `sched_yield`, `nanosleep`, `clock_nanosleep` returning scheduling outcomes (`Yield`, `Sleep(until)`); host tests

## 2. Interrupts and preemption

- [x] 2.1 AArch64: GICv3 from the device tree, virtual timer, IRQ from EL0 returns `Trap::Interrupt`; QEMU `virt` with `gic-version=3`; verify under QEMU
- [x] 2.2 x86_64: local APIC timer (x2APIC or xAPIC), PIC masked, vector 32 from ring 3 returns `Trap::Interrupt`; verify under QEMU `microvm` and `q35`
- [x] 2.3 Idle: wait for the next wake-up with the timer armed when nothing is runnable

## 3. Many cells

- [x] 3.1 Kernel `Host`: spawn, run, kill over `rhizo-sched`; H2 tests run through it
- [x] 3.2 Test cells and in-kernel tests: interleaving, ping-pong, spinning cell, sleep, same address, foreign mapping, fault next to a healthy cell, reused identifiers
- [x] 3.3 ASIDs (AArch64) and PCIDs (x86_64) with flush on cell end; verify the isolation tests on both

## 4. Benchmarks and documentation

- [x] 4.1 `cell.switch_ns` in `bench-cell` and `xtask bench`
- [x] 4.2 README roadmap (H3), AGENTS.md code map, `cargo xtask ci` on both architectures
