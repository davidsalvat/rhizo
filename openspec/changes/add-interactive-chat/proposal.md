# Proposal

## Why

The control plane (`add-control-plane`) runs, suspends, resumes and forks
cells from the host, and `rhizo run python-agent` replays a fixed agent
script through it. Nobody can yet *talk* to an agent in a cell: a cell's
standard input is the console, and reading it gives end of file. The
project owner wants to enter an agent running in a cell and converse with
it, as with a coding-agent command-line tool: type a message, the agent
answers (running tools inside the cell when it needs them), type the next.
That serves demos, real personal use of a sandboxed agent, and testing and
measuring Rhizo under an interactive load. Forking a conversation and
switching between its branches, and suspending it to pick it up later, are
what cells make cheap; the chat should expose both.

Scope agreed with the owner: local models only (no TLS), one conversation
on screen at a time, a fork in the middle of a conversation means switching
between branches, and the change stays small (it runs in parallel with the
rest of the proof of concept).

## What Changes

- **Standard input from the host.** A cell started with `input` gets a pipe
  on descriptor 0 whose only write end the host holds (in the kernel, not
  as a descriptor of the cell). New control requests `Input { cell, data }`
  (written whole or refused, at most 64 KiB, the pipe's capacity) and
  `CloseInput { cell }` (end of file once the rest is read). A thread
  blocked reading descriptor 0, or polling it, wakes up. The pipe belongs
  to the cell's files: a suspended cell keeps its unread input and its open
  host end in its snapshot; each branch of a fork, and each cell spawned
  from a template, gets a copy of its own, and the host decides which
  receives what. Protocol version 2 (`run` carries the flag).
- **Prompt output.** A watched cell's output is sent to the host as soon as
  the cell waits (an agent's answer, then its read of the next line), not
  only every 10 ms.
- **`rhizo chat`.** `rhizo chat --llm URL --model NAME` starts a chat cell
  (CPython with `agent_chat.py`); `rhizo chat N` talks to cell `N` again,
  resuming it if suspended. Each line typed is sent as `Input`; the
  agent's answer streams back, followed by the turn's time split between
  the model and Rhizo. Commands: `/fork K`, `/branches`, `/switch N`,
  `/suspend`, `/exit`, `/help`; Ctrl-D sends `CloseInput`. No line-editing
  library.
- **The agent's conversation mode** (`cells/python/agent_chat.py`, packed
  with the CPython cell next to `agent_loop.py`, whose `Model` it reuses):
  reads one message per line, keeps the whole conversation in the cell, and
  offers the model four tools that run in the cell: a Python snippet (output
  capped at 4000 characters, stopped after 10 s), read a file, write a
  file, list a directory. Answers without tool calls are shown whole. The
  scripted demo and its CI transcript are unchanged.
- **Fake model.** `hostnet::llm_server` answers the chat agent too: a
  snippet call after a user's message, then an answer quoting its output,
  deterministic.
- **Tests.** Protocol and file-system host tests (round trips, bounds,
  fork and snapshot of the pipe, malformed input), property tests extended
  to the new requests (they run in `cargo xtask check`), `rhizo` argument and
  chat-line parsing, and an end-to-end conversation in the `control` step
  of `cargo xtask test` on both architectures: two messages, `/fork 2`,
  `/branches`, `/switch`, a message, `/suspend`, `rhizo chat N` and a last
  message, its transcript checked.
- **Metric.** `chat.turn_us`: the Rhizo side of one interactive turn
  against the fake model.
- **Docs.** "Talk to an agent" in `docs/demo.md`, the metric in
  `bench/README.md`.

### Out of scope

TLS and hosted APIs, several conversations on screen, line editing and
history, streaming tokens as the model produces them (answers are shown
whole), remembering a conversation's branches across `rhizo chat` sessions
(the kernel does not record parentage), interrupting a running turn,
standard input for `rhizo run` and `rhizo attach`, UART interrupts.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `control-plane` (added by `add-control-plane`, not archived yet): cells'
  standard input, the input requests, prompt output, `rhizo chat`.
- `workloads`: the agent's conversation mode.
- `benchmarks`: `chat.turn_us`.

## Impact

`crates/rhizo-fs` (the host's input pipe: `host_input`, `push_input`,
`close_input`; its encoding), `crates/rhizo-snapshot` (format version 7),
`crates/rhizo-control` (version 2, `Input`, `CloseInput`, `Run::input`,
`MAX_INPUT`), `kernel` (`Host::input`, `Host::has_output`,
`src/control.rs`), `cli` (`chat.rs`, `args.rs`), `xtask` (`control.rs`:
chat markers, the end-to-end check and the metric; `hostnet.rs`: the fake
chat model; `cpython.rs`: the script), `cells/python/agent_chat.py`,
`agent_loop.py` (`Model.chat` takes the tools), `cells/expected.rs`,
`bench/README.md`, `docs/demo.md`. No new dependency. No benchmarked path
changes: the input pipe exists only in cells started with `input`, and the
output check runs only in the control kernel while someone watches a cell.
