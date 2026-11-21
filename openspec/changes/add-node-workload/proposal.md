# Proposal

## Why

The vision lists "a Node agent" as a template, and roadmap C.3 asks for
Node.js in a cell. Most agent tooling outside Python is JavaScript: an
official Node.js build is the second runtime agents expect. Dynamic linking
(`add-dynamic-linking`) gives cells the distribution's `ld.so`, file
mappings and `MAP_FIXED`; threads run on several CPUs; `epoll`, sockets and
the network card exist. What Node.js still lacks is unknown until it runs.

## What Changes

- **Node.js as a test input**: `xtask` downloads the official Node.js LTS
  build (v24.22.3, dynamically linked with glibc) for the cells'
  architecture when `RHIZO_NODE` is set, checks its pinned SHA-256, and
  packs it unmodified with `ld.so`, the libraries it needs and
  `cells/node/demo.js` into a cell file system (`node-files`). CI sets
  `RHIZO_NODE` and caches the download.
- **`--jitless` first**: Node.js runs with `--jitless`, which never asks
  for executable memory, so cells stay W^X. Running V8's JIT under W^X is
  designed here (design D6) and left to the next change.
- **What the kernel lacked**, each specified: `gettimeofday`,
  `clock_getres`, `capget` (no capabilities), `prctl` (`PR_SET_NAME`,
  `PR_GET_NAME`), `sched_getparam` and `sched_getscheduler` (the normal
  policy), and `io_uring_setup` declined on purpose (libuv falls back to
  `epoll`) without being reported as missing. A cell may hold 256 MiB of
  file data instead of 64 MiB (the `node` executable alone is 121 MiB),
  and the kernel keeps up to 256 MiB of mapped file pages so a second
  Node.js cell maps the first one's frames. On AArch64 cells may read the virtual
  counter directly (`CNTKCTL_EL1.EL0VCTEN` on every CPU), as Node.js's
  abseil does.
- **Kernels with Node.js**: a `node` kernel feature embeds `node-files`
  (over 100 MiB) only where asked: with `ktest`, a test kernel that runs
  the Node.js tests (`cargo xtask test --only ktest-node`, part of
  `cargo xtask test`, in a VM of 1 GiB and 2 CPUs); with `bench-cell`, a
  benchmark kernel that measures Node.js.
- **Tests**: Node.js answers a tool call (JSON, regular expressions,
  closures) as on Linux, and asks the host's fake model over HTTP with
  `node:http`, through the network card; a Node.js cell suspended after
  its warm-up resumes, forks and serves as a template; host tests of the
  new calls; the differential check runs the same files natively.
- **Metrics**: `workload.node_cold_us` (spawn to exit) and
  `workload.node_warm_us` (from a template frozen after the warm-up), and
  `workload.node_cold_us` for guest `linux-process` in the same
  Firecracker VM.

### Out of scope

The JIT (design D6: the next change), signals (`rt_sigaction` still
succeeds without delivering), `io_uring`, `/proc` (Node.js does without
`/proc/self/maps` and `/proc/self/exe`), npm and native add-ons.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `linux-abi`: the calls Node.js makes.
- `files`: 256 MiB of file data per cell.
- `isolation`: the counters a cell may read.
- `workloads`: Node.js in a cell.
- `benchmarks`: Node.js workload metrics.
- `comparisons`: Node.js as a Linux process in the same VM.
- `kernel-testing`: the Node.js tests in their own VM.

## Impact

`crates/rhizo-abi` (the new calls), `crates/rhizo-fs` (`DATA_MAX`),
`kernel/src/cell.rs` (`KEPT_PAGES`, `node_files`, the `rhizo-node`
benchmark line), `kernel/src/tests.rs`, `kernel/build.rs` and
`kernel/Cargo.toml` (feature `node`), `cells/node/demo.js`,
`cells/expected.rs`, `xtask/src/{tools,dynamic,differential,bench,compare,lib}.rs`,
`bench/host/linux_node.c`, `bench/README.md`, `.github/workflows/ci.yml`.
Node.js (MIT, its bundled dependencies under the licenses its `LICENSE`
lists) and the runner's `libstdc++` and `libgcc_s` (GPL-3.0-or-later with
the GCC Runtime Library Exception) are test inputs run unmodified, never
copied into the repository. No existing metric changes by design: kernels
without the `node` feature embed nothing more, and the larger bounds on
file data and kept pages change no benchmarked path.
