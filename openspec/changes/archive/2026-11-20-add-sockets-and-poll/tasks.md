# Tasks

## 1. Library crates

- [x] 1.1 `rhizo-sched`: a waiting set (deadline or none), woken all at once or at its deadline; verify host tests and a property test (never early, never lost)
- [x] 1.2 Create `rhizo-net` (D1, D2, D4, D6): loopback TCP and UDP, Unix stream sockets by name number, readiness, options, change counter, fork and snapshot records; add it to `xtask/src/deps.rs` and the AGENTS.md code map; verify `cargo test -p rhizo-net`
- [x] 1.3 `rhizo-fs`: pipes, event counters, `epoll` instances, files of type socket, socket descriptors, `O_NONBLOCK`; encoding; verify host tests (round trip, corrupt references, fork copies)
- [x] 1.4 `rhizo-abi`: pipe, event, poll, select, epoll and socket calls; blocking by restart (D3); verify host tests on both call conventions
- [x] 1.5 `rhizo-snapshot`: format version 4 with the sockets section; verify host tests (round trip, bad record)

## 2. Kernel and cells

- [x] 2.1 Kernel: one `Net`, waiting cells woken on its changes and deadlines, sockets released when a cell ends, forked (D4), suspended and resumed; verify `cargo xtask test --only ktest` on both architectures
- [x] 2.2 Test cells: `libc_net` (C, differential against Linux), `loopback` (two cells over TCP), `netstate` (sockets across suspend and fork); in-kernel tests for every scenario of `networking`; verify `cargo xtask test` on both architectures

- [x] 2.3 Copy-on-write of a 2 MiB page falls back to 4 KiB pages when no 2 MiB block is free (D8); verify `cargo test -p rhizo-mm` and the MicroPython in-kernel test on both architectures

## 3. Benchmarks and documentation

- [x] 3.1 `cell.pipe_rtt_ns`, `cell.loopback_rtt_us` in `bench-cell` and `xtask/src/bench.rs`, described in `bench/README.md`; verify `cargo xtask bench --runs 1` locally
- [x] 3.2 AGENTS.md (code map; the README follows in the archive pull request); verify `cargo xtask ci` on both architectures and `openspec validate add-sockets-and-poll --strict`
