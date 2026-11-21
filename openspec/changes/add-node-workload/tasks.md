# Tasks

## 1. Test input

- [x] 1.1 `tools::node` (pinned v24.22.3 for both architectures, SHA-256 checked), the `node-files` tree in `xtask/src/dynamic.rs` (D1), `cells/node/demo.js`, `NODE_ARGV`, `NODE_STDOUT` and `NODE_MODEL_LINE` in `cells/expected.rs`, and the differential check running the files natively with and without the fake model; verify `RHIZO_NODE=1 cargo xtask test --only differential` on x86_64

## 2. Kernel

- [x] 2.1 `gettimeofday`, `clock_getres`, `capget`, `prctl`, `sched_getparam`, `sched_getscheduler`, `io_uring_setup` declined (D2), with a host test of each on both architectures' numbers; verify `cargo test -p rhizo-abi node_start_up_calls`
- [x] 2.2 `DATA_MAX` of 256 MiB and `KEPT_PAGES` of 65536 (D3); verify `cargo test -p rhizo-fs file_data_is_limited_per_cell` and `cargo xtask check`
- [x] 2.3 Feature `node` (`kernel/build.rs` embeds `node-files` only there; with `ktest` the Node.js suite alone), `node_files`, in-kernel tests `node_answers_as_on_linux` and `a_node_cell_suspends_resumes_and_forks` (D4), step `ktest-node` (1 GiB, 2 CPUs) in `xtask`; verify `RHIZO_NODE=1 cargo xtask test --only ktest-node` on x86_64 and on AArch64 (QEMU)

- [x] 2.4 `CNTKCTL_EL1` set to `EL0VCTEN` on every CPU (D8), the `ticks` test cell in `cells/expected.rs` and `a_cell_reads_the_counter` in both architectures' tests; verify `RHIZO_NODE=1 cargo xtask --arch aarch64 test --only ktest-node` fails without it (system register trap) and passes with it; the PMU sampler stopped on every CPU (D8), so the test also passes after the shared suite: verify `cargo xtask --arch aarch64 test --only ktest-smp`

## 3. Benchmarks

- [x] 3.1 `rhizo-node` line of the `node` benchmark kernel, `measure_node` and `--workload node` in `xtask/src/bench.rs` with a parser test (D5); verify `RHIZO_NODE=1 cargo xtask bench --workload node --vmm qemu-microvm --runs 1`
- [x] 3.2 `bench/host/linux_node.c` and `compare::node_in_vm` (the Linux process in Firecracker, cold only) with a parser test; verify `cargo test -p xtask node_line` and the C program run natively in a root holding the packed files
- [ ] 3.3 `bench/README.md` describes the metrics; CI sets `RHIZO_NODE` and caches `target/tools`; verify the CI benchmark gate passes with the new metrics present and no metric worse

## 4. Documentation

- [ ] 4.1 `AGENTS.md` names the `ktest-node` step, `RHIZO_NODE` and `cells/node`; the JIT design is recorded for the next change (D6); verify `openspec validate add-node-workload --strict`
