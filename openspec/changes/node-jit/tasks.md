# Tasks

## 1. Page tables

- [ ] 1.1 `PageFlags::WRITE_OR_EXEC` in both entry formats (D2); verify `cargo test -p rhizo-mm write_or_exec`
- [ ] 1.2 `resolve_exec`, and `resolve_write` for write-or-execute pages, alone, after a fork, in clones (D3, D6); verify `cargo test -p rhizo-mm wx`
- [ ] 1.3 Property test: no leaf both writable and executable after any sequence of operations (D2); verify `cargo test -p rhizo-mm model`

## 2. Cell memory and system calls

- [ ] 2.1 `space::WxRanges` and backing a page of a range (D5); verify `cargo test -p rhizo-cell wx`
- [ ] 2.2 `mmap`, `mprotect`, `munmap` and user-memory access with write-or-execute memory, `Context::wx`, `Pager::back_wx` (D5); verify `cargo test -p rhizo-abi wx` and `cargo test -p rhizo-abi hostile`

## 3. Snapshots

- [ ] 3.1 Format version 8: the page bit and the ranges, checked on decoding (D7); verify `cargo test -p rhizo-snapshot`

## 4. Kernel

- [ ] 4.1 `exec_denied` on both architectures, `SCTLR_EL1.UCT`/`UCI` on AArch64 (D3, D8); verify `cargo xtask test --only ktest` on both architectures
- [ ] 4.2 Fault path (one CPU and gangs), ranges in `Live`, fork, templates, suspend and resume, `Count::WxFlips` (D3-D7); verify the in-kernel `libc_jit` tests on both architectures, `--only ktest` and `--only ktest-smp`
- [ ] 4.3 `libc_jit` cell and its tests: plain, forked, suspended and resumed eagerly and lazily, threads, bounce (D9); verify `cargo xtask test` on both architectures and the differential check

## 5. Node.js

- [ ] 5.1 The Node.js tests and differential check without `--jitless` (D9); verify `RHIZO_NODE=1 cargo xtask test` and `RHIZO_NODE=1 cargo xtask --arch x86_64 test`
- [ ] 5.2 `workload.node_jit_cold_us`, `workload.node_jit_warm_us`, `workload.node_jit_flips` (D10); verify `cargo test -p xtask node` and `RHIZO_NODE=1 cargo xtask --arch x86_64 bench --workload node --runs 1`

## 6. Gates and documents

- [ ] 6.1 Hot paths unchanged: `-icount` gates; verify `cargo xtask bench --env ci --check` in CI
- [ ] 6.2 `bench/README.md`, `docs/security.md` (mmap, mprotect, snapshot format), figures in the pull request; verify by reading
- [ ] 6.3 Everything CI runs; verify `cargo xtask check` and CI
