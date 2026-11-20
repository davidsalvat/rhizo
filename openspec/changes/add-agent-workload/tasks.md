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
- [ ] 3.3 `bench/README.md` documents the metrics; the `ci` baseline gains the agent metrics from a CI run; verify `cargo xtask ci` and `openspec validate add-agent-workload --strict`
