# Tasks

## 1. `sysinfo`

- [ ] 1.1 `sysinfo` reports the cell's limit, its use and its threads, from `Frames::metered` (D1); verify `cargo test -p rhizo-abi sysinfo` and the in-kernel `sysinfo_shows_a_cell_only_itself` on both architectures

## 2. Ports

- [ ] 2.1 `Ports`, `Net::enter`, `Net::leave`; `ephemeral` walks the entered cursor from its keyed start (D2); verify `cargo test -p rhizo-net ephemeral`
- [ ] 2.2 `Abi` keeps the cell's next port and enters it for the calls that can bind (D2); verify `cargo test -p rhizo-abi another_cells_binds` and the in-kernel `ephemeral_ports_do_not_show_other_cells_binds` on both architectures

## 3. Page cache

- [ ] 3.1 `Page::is_held_elsewhere`; `PageCache::share`, `own_copy`, `trim`; a full cache drops only dead pages and gives copies; `a_full_cache_starts_over` replaced (D3, D5); verify `cargo test -p rhizo-fs held_elsewhere` and `cargo test -p rhizo-abi pages::`
- [ ] 3.2 `KernelPages` copies from the cell's frames; `drop_images` trims (D3); verify the in-kernel `memory_pressure_keeps_the_pages_other_cells_map` on both architectures

## 4. Hostile tests and documents

- [ ] 4.1 `observer` and the three in-kernel tests (D5); verify `cargo xtask test` and `cargo xtask --arch x86_64 test`
- [ ] 4.2 Findings S24 to S26 in `docs/security.md`, `docs/audit-scope.md` updated; verify by reading

## 5. Benchmarks and CI

- [ ] 5.1 Every `-icount` gate (D6); verify `cargo xtask bench --icount-only --env ci --check` on x86_64 and `--env ci-arm` on AArch64
- [ ] 5.2 Everything CI runs; verify `cargo xtask check`, `openspec validate close-cross-cell-leaks --strict` and CI
