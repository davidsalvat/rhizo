## ADDED Requirements

### Requirement: FP switch metric
`cargo xtask bench` SHALL report `cell.switch_fp_ns`: the time per switch
between two cells that yield to each other while holding values in every
FP/SIMD register (`fp_regs`), the run of both to their exit minus the same
run without yields, over the switches. Like every figure the kernel
times, it SHALL also be counted in instructions under `-icount`
(`cell.switch_fp_insns`) and by the guest's PMU where there is one.

#### Scenario: Metric present
- **WHEN** `cargo xtask bench` runs
- **THEN** the results contain `cell.switch_fp_ns` for guest `rhizo` on each VMM, and `cell.switch_fp_insns` on the `-icount` VMM
