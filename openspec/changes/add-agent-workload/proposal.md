# Proposal

## Why

The proof of concept succeeds when a real agent runs on Rhizo end to end,
on both architectures and every VMM, using what makes cells different
(network, fork, suspend and resume), and is at least as fast as the same
agent as a Linux process in the same VM (README roadmap, stage "a real
agent", line A.4). Every piece exists alone: CPython runs as a cell, the
network card reaches the host, TCP connections survive a suspend, cells
fork copy-on-write. Nothing yet runs them together the way an agent does,
so nothing proves they compose, and no figure says what one agent turn
costs on Rhizo against Linux.

## What Changes

- **The fake model on the host** (`xtask`, from `add-reference-workloads`)
  is reused: every guest network now gets it (`llm=ADDR:PORT` next to
  `echo=`), it keeps connections alive, and it always asks for a tool call
  whose arguments derive from the whole conversation, so a conversation
  always gets the same answers. Its delay stays configurable
  (`--llm-delay-ms`, 0 by default).
- **The agent**, `cells/python/agent_loop.py`, a CPython program: each
  turn sends the conversation over one kept-alive HTTP/1.1 connection,
  parses the JSON answer, runs the requested tool (a small computation and
  a file written and read back) and appends the result. At a decision
  point it forks into three branches that each explore a different tool
  result over a connection of their own.
- **On Rhizo** the host suspends and resumes the agent between turns (its
  connection to the model survives) and makes the branches with its own
  fork: it freezes the agent at the decision point as a template and starts
  one cell per branch, telling each its branch in a file. The kernel gains
  two host operations for that: whether a cell sleeps (a safe point to
  suspend or freeze it) and writing a file into a live cell.
- **CPython** gains its `_socket`, `select`, `array`, `unicodedata` and
  hash modules (built in, static); the packed standard library covers both
  scripts.
- **`ioctl`** implements `FIONBIO`, `FIOCLEX` and `FIONCLEX` (a Python
  socket with a timeout needs `FIONBIO`); other requests stay `ENOTTY`.
- **The same agent as a Linux process** in the same Firecracker VM: the
  guest's `/init` brings its network card up, and the agent forks its
  branches with `os.fork`.
- **Any OpenAI-compatible model**: the agent takes the API's base URL and
  model name as arguments, reads chunked answers and survives answers that
  call no tool, so the fake model and Ollama or `llama-server` are
  interchangeable.
- **A live demo**, `cargo xtask demo agent --llm URL --model NAME [--vmm]`
  (not run in CI), and `docs/demo.md`, linked from the README.
- **Metrics** in `cargo xtask bench`: `workload.agent_cpython_turn_us`,
  `workload.agent_suspend_resume_us`, `workload.agent_fork_us` for Rhizo on
  every VMM with a network card, and `workload.agent_cpython_turn_us` and
  `workload.agent_fork_us` for the Linux process in Firecracker.
- **Tests**: an in-kernel test runs a few turns with a suspend and lazy
  resume after each and the fork, and checks the transcript against the one
  recorded from Linux, on QEMU (both architectures), Firecracker and Cloud
  Hypervisor in CI; the differential check runs the agent as a Linux
  process against the same fake model and checks the same transcript.

### Out of scope

- **TLS.** The agent speaks plain HTTP to the host (design D2); HTTPS with
  a static OpenSSL in CPython's `ssl` module is a follow-up change.
- **Fork inside the cell** (`os.fork` creating a process in a cell): cells
  have no processes; the branches are the host's fork (design D3).
- **Suspend and resume of the Linux process** (CRIU with established TCP
  connections, design D5): not measured; the existing CRIU restore figure
  stays the reference.
- **AArch64 figures**: AArch64 runs are emulated in CI (no KVM on the ARM
  runners), so the agent is tested there but not benchmarked.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workloads`: a real agent end to end.
- `benchmarks`: agent metrics.
- `linux-abi`: descriptor `ioctl`s.
- `comparisons`: the agent as a Linux process in the same VM.

## Impact

`crates/rhizo-abi` (`ioctl`), `AGENTS.md` (`zlib1g-dev`),
`.github/workflows/ci.yml` (results published compact, in a step of their
own: they outgrew one step's ten annotations), `cells/python/agent_loop.py` (new), `cells/python/pack_stdlib.py` (several
scripts), `cells/expected.rs`, `crates/rhizo-core` (boot option `llm=`),
`kernel/src/cell.rs` (`Host::asleep`, `Host::write_file`, the agent driver
and its figures), `kernel/src/tests.rs`, `xtask/src/{hostnet,cpython,
differential,compare,bench}.rs`, `bench/host/linux_cpython.c`,
`bench/README.md`, `bench/baseline.json`. CPython's build recipe changes,
so CI rebuilds it once per architecture.
