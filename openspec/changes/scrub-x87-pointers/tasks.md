# Tasks

## 1. Findings

- [x] 1.1 Probe the CPUID and `FXRSTOR` behavior of QEMU's `-cpu max` under TCG (D1); verify with a debug build of `x87_pointers_never_survive_a_load` under `cargo xtask --arch x86_64 test --only ktest` (recorded in D1)

## 2. Spec

- [x] 2.1 Proposal, design and the isolation delta; verify `openspec validate scrub-x87-pointers --strict`

## 3. Kernel

- [x] 3.1 `amd_like` shared with `kpti_needed`, `fxsave_leaks` and the flag set in `init_tags` (D1), the scrub in `fp_load` (D2); verify `cargo xtask check`
- [x] 3.2 In-kernel tests `the_x87_scrub_follows_the_cpu` and `x87_pointers_never_survive_a_load` (D4); verify `cargo xtask --arch x86_64 test --only ktest` (and that the second fails with the scrub off) and `--only ktest-smp`
- [x] 3.3 `docs/security.md`: the scrub under per-cell state; verify by reading

## 4. Gates

- [x] 4.1 Cost (D3); verify `cargo xtask --arch x86_64 bench --icount-only --env ci --check`
- [x] 4.2 Everything; verify `cargo xtask check`, `cargo xtask --arch x86_64 test` and `cargo xtask test`
