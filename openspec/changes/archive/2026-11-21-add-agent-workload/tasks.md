# Tasks

## 1. Host side

- [x] 1.1 `rhizo-core`: boot option `llm=ADDR:PORT` (from `add-reference-workloads`), tested next to `echo=`; verify `cargo test -p rhizo-core options`
- [x] 1.2 `xtask`: the fake model server of `add-reference-workloads` kept alive (`Connection: close` still closes) with deterministic tool calls, on every guest network; verify `cargo test -p xtask hostnet`

## 2. The agent

- [x] 2.0 `rhizo-abi`: `ioctl` `FIONBIO`, `FIOCLEX`, `FIONCLEX`; verify `cargo test -p rhizo-abi ioctl`
- [x] 2.1 CPython with `_socket`, `select`, `array`, `unicodedata` and the hash modules; `pack_stdlib.py` packs several scripts; `cells/python/agent_loop.py` in `/app`; verify a CPython build with `RHIZO_CPYTHON_SRC` and `cargo xtask test --only differential` (the agent as a Linux process prints `AGENT_TRANSCRIPT`)
- [x] 2.2 Kernel: `Host::asleep`, `Host::write_file`, `cell::agent`; in-kernel test `a_cpython_agent_talks_forks_and_survives_suspend`; verify `cargo xtask test --only ktest` and `--only ktest-smp` on both architectures, and the CI jobs on Firecracker and Cloud Hypervisor

## 3. Benchmarks and comparison

- [x] 3.1 `rhizo-agent` line in `bench-cell`, metrics `workload.agent_cpython_turn_us`, `workload.agent_cpython_model_us`, `workload.agent_suspend_resume_us`, `workload.agent_fork_us` in `xtask/src/bench.rs`; verify `cargo test -p xtask bench` and the CI benchmark job
- [x] 3.2 The agent as a Linux process in Firecracker (`linux_cpython.c` with a network card, `compare::cpython_in_vm`); verify `cargo test -p xtask compare` and the CI benchmark job's `linux-process` figures
- [x] 3.3 `bench/README.md` documents the metrics; the `ci` baseline gains the agent metrics from a CI run; verify `cargo xtask ci` and `openspec validate add-agent-workload --strict`

## 4. Live demo

- [x] 4.1 The agent takes a base URL and a model name, reads chunked answers, survives answers without tool calls; verify `cargo xtask test --only differential` (fake model transcript, and a model that only answers text in chunks)
- [x] 4.2 `bench=agent-demo`, `cell::agent` live output, `cargo xtask demo agent`; verify `cargo test -p xtask demo` and a run under emulation against a stand-in model
- [x] 4.3 `docs/demo.md`, linked from the README's "Try it"; verify `openspec validate add-agent-workload --strict`

## 5. Fixes

- [x] 5.1 `rhizo-snapshot`: a 2 MiB page restored as 4 KiB pages without a free 2 MiB run; kernel syncs code per page; verify `cargo test -p rhizo-snapshot` and the agent's in-kernel test after merging `add-large-image-pages`
- [x] 5.2 The agent's test waits for the card's connections to close; verify `cargo xtask test --only ktest` (the dynamic linking test after it counts the heap)
- [x] 5.3 The cell metrics in a 256 MiB VM; verify `cargo xtask bench --runs 1 --vmm qemu-microvm --no-reference` prints every cell figure, and the CI benchmark job
- [x] 5.4 `rhizo-net`: `TCP_NODELAY` set on a connection through the card applies at once; verify `cargo test -p rhizo-net no_delay` (fails without the fix) and the CI benchmark job's agent turns
