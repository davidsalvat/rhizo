# Tasks

## 1. Accounting

- [x] 1.1 `Quota`, `Metered` and `Cell::charge_cpu` in `rhizo_cell::quota` (D1, D2); verify `cargo test -p rhizo-cell quota`
- [x] 1.2 Descriptor and socket limits in `rhizo-fs` (D4); verify `cargo test -p rhizo-fs quotas`
- [x] 1.3 `getrlimit`/`prlimit64` report the quotas (D3); verify `cargo test -p rhizo-abi rlimits`
- [x] 1.4 Snapshot format version 6 with quotas and CPU time used (D3); verify `cargo test -p rhizo-snapshot`

## 2. Kernel

- [x] 2.1 Every frame taken for a cell metered, `Host::set_quota` (D1); verify the in-kernel `memory_quota_refuses_the_cell_not_the_others` on both architectures
- [x] 2.2 CPU time counted per slice, `Ending::OutOfCpu` (D2); verify the in-kernel `cpu_quota_ends_the_cell` on both architectures
- [x] 2.3 Quotas carried by forks, template copies and resumes (D3); verify the in-kernel `file_and_socket_quotas_travel_with_the_cell` on both architectures

## 3. Documentation and CI

- [x] 3.1 `docs/security.md` (O1 closed, quotas, what they do not cover); verify by reading
- [ ] 3.2 Everything CI runs; verify `cargo xtask ci` on both architectures and CI green
