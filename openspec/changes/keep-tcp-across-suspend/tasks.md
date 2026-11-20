# Tasks

## 1. Library crates

- [x] 1.1 `rhizo-net`: park connected and connecting stream sockets on export, re-attach on import (boot identity and key), reset otherwise, `unpark`; `Saved::Parked` in records; verify host tests (loopback, Unix pair and card connections across export and import with data sent while parked, other boot, second import, unpark, fork sequence `ECONNRESET` then 0 and `EPIPE`, record round trip)
- [x] 1.2 `rhizo-snapshot`: format version 6; verify `cargo test -p rhizo-snapshot` (a parked record round-trips through a snapshot)

## 2. Kernel and cells

- [x] 2.1 Kernel: boot identity, export parks, import re-attaches, `cell::close_parked`; verify `cargo xtask test --only ktest` on both architectures
- [ ] 2.2 Cells and in-kernel tests: `netstate` checks the reset sequence; `loopback twice`; tests for every new scenario of `networking` (loopback pair across suspend, host echo across suspend, second resume, discard, fork); verify `cargo xtask test` and `--only ktest-smp` on both architectures, and the host echo test on QEMU and Firecracker in CI

## 3. Benchmarks and documentation

- [ ] 3.1 `cell.tcp_resume_us` in `bench-cell` and `xtask/src/bench.rs`, described in `bench/README.md`; verify `cargo xtask bench --runs 1` locally and the CI benchmark gate
- [ ] 3.2 Verify `cargo xtask ci` and `openspec validate keep-tcp-across-suspend --strict`
