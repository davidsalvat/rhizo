# Tasks

## 1. Test inputs

- [ ] 1.1 `xtask/src/packages.rs`: the dynamically linked CPython built from the pinned tarball and installed into a staging tree (D1), the pinned wheels downloaded, checked and installed into its `site-packages` (D2); `pack_stdlib.py` packing `site-packages` modules, package data, `.libs` and `.dist-info`, and script arguments; `dynamic::pack` using libraries vendored under `$ORIGIN` (D3); `packages-files` embedded by `kernel/build.rs`; the CI cache key covers `packages.rs`; verify `cargo xtask --arch x86_64 test --only differential` builds the tree natively
- [ ] 1.2 `cells/python/numpy_demo.py`, `cells/python/requests_demo.py`, `NUMPY_*` and `REQUESTS_*` in `cells/expected.rs`, and the differential check running both natively against the host's fake model; verify `cargo xtask --arch x86_64 test --only differential`

## 2. Kernel tests

- [ ] 2.1 In-kernel tests `numpy_from_a_wheel_computes_as_on_linux`, `requests_gets_from_the_host_as_on_linux` and `a_cell_that_imported_numpy_suspends_resumes_and_forks` (D5); test VMs with 1 GiB, the memory pressure test sized by free memory (D4); verify `cargo xtask test --only ktest` and `cargo xtask test --only ktest-smp` on both architectures

## 3. Benchmarks and documentation

- [ ] 3.1 `rhizo-numpy` line of the `bench-cell` kernel and `workload.numpy_cold_us`, `workload.numpy_warm_us` in `xtask/src/bench.rs` with a parser test; the cell benchmark VM with 512 MiB (D4); verify `cargo test -p xtask numpy`
- [ ] 3.2 numpy as a Linux process in the comparison's Firecracker guest (`bench/host/linux_numpy.c`, `bench/host/numpy_forkserver.py`, `compare::numpy_in_vm`) with a parser test (D6); verify `cargo test -p xtask numpy_line` and the CI benchmark gate passes with the new metrics present and no metric worse
- [ ] 3.3 `bench/README.md` describes the metrics; `AGENTS.md` code map names `packages.rs`; verify `openspec validate add-pip-packages --strict`
