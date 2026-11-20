# Tasks

## 1. Build

- [ ] 1.1 `xtask/src/cpython.rs`: pinned tarball checked against its SHA-256, static build with `Setup.local`, recipe stamp, native only, skipped without `RHIZO_CPYTHON_SRC` (D1); CI downloads the tarball and caches the build in `ci`, `ci-arm64` and `bench`; verify the build passes in CI on both architectures
- [ ] 1.2 Standard library packing (`cells/python/pack_stdlib.py`) and encoding as a cell file system, `xtask` depends on `rhizo-fs` (D2); host test of the encoding; verify `cargo test -p xtask cpython`

## 2. Workload

- [ ] 2.1 `cells/python/agent_demo.py` and `CPYTHON_ARGV`/`CPYTHON_STDOUT` in `cells/expected.rs`; differential check with an empty environment and no standard error (D3); verify the differential check passes in CI on both architectures

## 3. Kernel

- [ ] 3.1 The kernel embeds `cpython` and `cpython-files` (empty when not built), decodes the files once and clones them per cell (D2); verify `cargo xtask test` without CPython says it did not run
- [ ] 3.2 System calls CPython needs that cells lack, each with host tests in `rhizo-abi`; verify `cargo test -p rhizo-abi` and no `unimplemented syscall` in the CPython test's console in CI
- [ ] 3.3 In-kernel test `cpython_in_a_cell`: cold answer as on Linux, three forks of the warm template answer; verify `cargo xtask test` in CI on both architectures

## 4. Benchmarks and documentation

- [ ] 4.1 `workload.cpython_cold_us` and `workload.cpython_warm_us` (D3): `bench-cell` line, parsing in `xtask/src/bench.rs` with a unit test, `bench/README.md`; verify the CI bench job reports them
- [ ] 4.2 CPython as a Linux process in the same Firecracker VM (D4): `bench/host/linux_cpython.c`, `bench/host/cpython_forkserver.py`, nested paths in `initramfs_with`, host tests of the initramfs and the result line; verify the CI bench job reports both metrics for `linux-process` on `firecracker`
- [ ] 4.3 README section "A real interpreter" with the CPython figures, `bench/README.md` metrics and comparison rows, `AGENTS.md` code map; verify `cargo xtask check`
