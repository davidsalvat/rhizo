# Tasks

## 1. Libraries

- [x] 1.1 `rhizo_prof::samples` (buffer with reserved room, lost count, folding, lines) (D4); host tests; verify `cargo test -p rhizo-prof`
- [x] 1.2 `Fdt::pmu_interrupt`; host test on the QEMU `virt` device tree; verify `cargo test -p rhizo-core fdt`

## 2. Kernel

- [x] 2.1 `arch::pmu::{start_sampling, stop_sampling, rearm}`, `Trap::Sample`, vector 34 and `LVTPC` on x86_64, the PMU PPI on AArch64, `irq::set_tick_ns` (D1–D3); verify `cargo xtask test --only ktest` on both architectures
- [x] 2.2 `prof` sampling, samples on `Trap::Sample` and timer ticks, slices of 101 ticks while the timer samples, re-arm after idling; feature `sampling` (in `ktest`); in-kernel test `samples_show_where_a_cell_spins` (timer on x86_64 TCG, PMU on AArch64 TCG); verify `cargo xtask test --only ktest` on both architectures
- [x] 2.3 Breakdowns with samples in `sampling` builds (five rounds); verify the `bench-cell` kernel's counts under `-icount` are unchanged

## 3. xtask and documentation

- [x] 3.1 `cargo xtask profile` (parsing, `nm` symbols, folded stacks per workload and per system call, summary) (D5); unit tests; verify `cargo xtask --arch x86_64 profile --accel tcg` and `cargo xtask --arch aarch64 profile`
- [ ] 3.2 `bench/README.md`; `cargo xtask ci` on both architectures; CI green
