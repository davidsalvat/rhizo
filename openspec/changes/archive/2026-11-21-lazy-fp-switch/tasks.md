# Tasks

## 1. Profile

- [x] 1.1 `cell.switch_fp_ns` and the `fp_regs` cell, measured on the code before the change next to `cell.switch_insns`, `cell.syscall_insns` (D4, design *Context*); verify `cargo xtask --arch x86_64 bench --icount-only` and `cargo xtask --arch aarch64 bench --icount-only` print `cell.switch_fp_insns`

## 2. Cells

- [x] 2.1 `Registers::fp_used` at offset 280, `FpState::initial` const (D3); verify `cargo test -p rhizo-cell` (layout test) and `cargo test -p rhizo-snapshot`
- [x] 2.2 `fp_regs.c` and `fp_peek.rs`, in `cells/expected.rs` (D4); verify the differential check in `cargo xtask check` and a native run of `fp_regs 3 2000 4 3`

## 3. Kernel

- [x] 3.1 `fp_enter`, `fp_first_use`, saves where a thread stops running, owner `FP_CLEAN`/`FP_STALE`, trapping tied to the owner, x86_64 `fp_save` keeping `CR0.TS` (D1); verify `cargo xtask check`
- [x] 3.2 In-kernel tests: registers after a non-FP cell, cells and threads next to hostile readers, suspends, resumes and forks (D2, D4); verify `cargo xtask test --only ktest` and `--only ktest-smp` on both architectures, and that the register test fails on the code before the change

## 4. Benchmarks and documents

- [x] 4.1 `cell.switch_fp_ns` in `xtask` and `bench/README.md`; new `bench/baseline.json` entries for `cell.switch_fp_insns` only (D4); verify `cargo test -p xtask` and `cargo xtask bench --icount-only --check` on both architectures
- [x] 4.2 `docs/security.md`: per-cell state, LazyFP under *Speculation*, O3 (D2); verify by reading
- [x] 4.3 `openspec validate lazy-fp-switch --strict`; `cargo xtask --arch x86_64 test` and `cargo xtask test`
