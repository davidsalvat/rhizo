# Demo: an agent in a cell, with your own model

`cargo xtask demo agent` boots Rhizo and runs a real agent in a cell: CPython
3.13 with `cells/python/agent_loop.py`, talking over HTTP to a model you run
on your machine through any OpenAI-compatible `/v1/chat/completions` API
with tool calls (Ollama, llama.cpp's `llama-server`). It runs three turns
(the model asks for a tool, the cell runs it: a computation and a file
written and read back), with the host suspending and resuming the cell
between turns while its connection to the model stays open; then, at a
decision point, the host freezes the agent and forks it into three branches
that each explore a different tool result over a connection of their own.

Every step is printed as it happens, with its time split between the model
and Rhizo:

```text
[rhizo] cell: CPython with `agent_loop.py`, model `http://10.0.2.2:18080`
[agent] user: Explore the sums and keep notes.
[turn 1] model (11.9 ms): I think the sums look fine.
[turn 1] no tool call: asked again (python 7.6 ms)
[rhizo] suspended between turns in 39231 us (15467 KiB snapshot, connection parked), resumed (Lazy) in 6809 us
[turn 2] model (20.0 ms): calls compute(n=900, seed=3)
[turn 2] tool: value=4050, 1 notes (python and tool 60.9 ms)
...
[agent] decision point: asking for a fork
[branch 0] running
[rhizo] froze the agent at its decision point and forked 3 branches (copy-on-write, own connections): all running after 117202 us
[branch 1] explores variant 1 of the last result: value=4003
[branch 2 turn] model (21.4 ms): calls compute(n=1200, seed=3)
...
rhizo-demo done in 1732 ms: 3 suspend and resume (median 36090 us), fork into 3 branches 117202 us
```

(Under emulation, against a stand-in model that answers at once.)

`[agent]`, `[turn N]` and `[branch N]` lines come from the agent in the
cell (model time: request sent until the whole answer is read; Python time:
the rest of the turn); `[rhizo]` lines come from the kernel's host.

This is a demo: CI runs the same agent against a deterministic fake model
instead (`workload.agent_*` metrics in `bench/README.md`), so no model is
downloaded there.

## What you need

- Everything in the README's "Try it" section, plus `zlib1g-dev`.
- CPython's pinned source, built into the cell on first use (a few
  minutes, then cached under `target/cpython`). It is built natively, so
  the kernel's architecture must be the host's: `--arch x86_64` on an
  x86_64 machine, `--arch aarch64` (the default) on an ARM one.

```bash
curl -LO https://www.python.org/ftp/python/3.13.18/Python-3.13.18.tar.xz
export RHIZO_CPYTHON_SRC=$PWD/Python-3.13.18.tar.xz
```

- A model that can call tools. Small ones work: Qwen2.5 1.5B Instruct,
  Llama 3.2 3B Instruct. When a model answers without calling the tool
  (small ones sometimes do), the turn still completes: its reply is shown
  and the agent asks again.

## With Ollama

```bash
ollama serve &
ollama pull qwen2.5:1.5b
cargo xtask --arch x86_64 demo agent --llm http://127.0.0.1:11434 --model qwen2.5:1.5b
```

## With llama.cpp

`--jinja` makes `llama-server` apply the model's chat template, which is
what turns its output into tool calls:

```bash
llama-server -m qwen2.5-1.5b-instruct-q4_k_m.gguf --jinja --port 8080 &
cargo xtask --arch x86_64 demo agent --llm http://127.0.0.1:8080 --model qwen2.5
```

(`llama-server` serves the one model it loaded, whatever `--model` says.)

## Which VMM, and where the guest finds the model

`--vmm` picks the virtual machine (default: QEMU `microvm` on x86_64, QEMU
`virt` on AArch64). Give `--llm` as the host sees the model; `xtask`
rewrites a loopback address to the host's address as the guest sees it:

| VMM | Guest network | The host, seen from the guest | The model must listen on |
|---|---|---|---|
| `qemu-microvm`, `qemu-q35`, `qemu-virt` | QEMU user network (DHCP) | `10.0.2.2` | `127.0.0.1` is enough |
| `firecracker` | tap device `rhizotap0` | `172.16.0.1` | `172.16.0.1` or `0.0.0.0` |
| `cloud-hypervisor` | tap device `rhizotap1` | `172.16.1.1` | `172.16.1.1` or `0.0.0.0` |

For Firecracker and Cloud Hypervisor, make the model listen beyond the
loopback: `OLLAMA_HOST=0.0.0.0 ollama serve`, or `llama-server --host
0.0.0.0`. Both need KVM, and the tap devices need password-less `sudo`
(`xtask` creates them). Another IPv4 address in `--llm` is passed as it is:
QEMU's user network reaches other machines; the tap devices have no route
beyond the host.

QEMU uses KVM when `/dev/kvm` is usable; `--accel tcg` forces emulation.

## What to expect

The model's time dominates every turn: from a few hundred milliseconds to
several seconds per answer for a 1.5B model on a CPU, more for longer
conversations. Rhizo's own steps, measured by the kernel:

| Step | With KVM (CI's figures, fake model) | Under emulation (TCG) |
|---|---|---|
| Python and tool, per turn | about 0.1 ms | 5 to 100 ms |
| Suspend (15 MiB snapshot) plus resume | about 2.7 ms (eager resume) | 30 to 45 ms (lazy resume) |
| Fork into 3 branches until all run | about 1.2 ms | about 120 ms |

Under emulation everything in the guest is 10 to 100 times slower, but the
model runs natively on the host, so the demo stays usable.

## Caveats

- **Plain HTTP only.** The cell's CPython has no TLS yet (design D2 of
  `add-agent-workload`), so hosted APIs over HTTPS are out of reach; run
  the model locally, or put a plain-HTTP proxy next to it.
- **IPv4 addresses**, not host names, in `--llm`.
- **The guest polls its network card.** Waiting for the model costs no CPU
  in the host's sense only between polls; a turn's model time includes up
  to one polling period.
- **One run.** Three turns, three branches, then the VM stops; the cell's
  files and snapshots are not kept.
- **Memory.** The guest has 128 MiB: the agent and its three branches fit
  (they share their pages copy-on-write), a much longer conversation may
  not.
