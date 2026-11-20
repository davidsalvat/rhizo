# Tasks

## 1. x86_64

- [x] 1.1 Entry pages on pages of their own, per-CPU CR3 slots, CR3 switches in the entry and return paths (D2); verify `cargo xtask --arch x86_64 test --only ktest`
- [x] 1.2 Entry tables, `kpti_needed`, `set_kpti`, the cells' kernel half (D1, D2); verify the in-kernel `kpti_leaves_cells_only_the_entry_pages`
- [x] 1.3 Kernel flushes mark the cell stale everywhere (D3); verify the in-kernel `cells_run_alike_with_and_without_kpti` and the fork tests with KPTI on, on 1 and 4 CPUs (`--only ktest-smp`)

## 2. AArch64

- [x] 2.1 E0PD on every CPU that has it, report otherwise (D4); verify `cargo xtask test --only ktest` and `--only ktest-smp`

## 3. Cost

- [x] 3.1 `syscalls` cell, `rhizo-kpti` line and the four metrics (D5); verify `cargo test -p xtask kpti` and a local `cargo xtask --arch x86_64 bench --runs 1 --vmm qemu-microvm --no-reference`
- [x] 3.2 CI numbers before and after in the pull request, `bench/README.md` and `docs/security.md` (O3) updated; verify by reading

## 4. CI

- [x] 4.1 Everything CI runs; verify `cargo xtask ci` on both architectures and CI green
