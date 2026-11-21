# Demo: an agent in a cell, with your own model

`rhizo` is Rhizo's control-plane tool: `rhizo up` boots a Rhizo VM whose
kernel serves requests, and the other commands run, list, suspend, resume,
fork and kill its cells. The kernel only executes; what to run and when to
suspend or fork is decided by `rhizo` on the host.

`rhizo run python-agent` runs a real agent in a cell: CPython 3.13 with
`cells/python/agent_loop.py`, talking over HTTP to a model you run on your
machine through any OpenAI-compatible `/v1/chat/completions` API with tool
calls (Ollama, llama.cpp's `llama-server`). It runs three turns (the model
asks for a tool, the cell runs it: a computation and a file written and
read back), with `rhizo` suspending and resuming the cell after every turn
while its connection to the model stays open; then, at the agent's
decision point, `rhizo` forks it into three branches that each explore a
different tool result over a connection of their own.

Every step is printed as it happens, with its time split between the model
and Rhizo:

```text
$ rhizo up
rhizo is up: qemu-microvm (x86_64), 256 MiB, 1 CPU, kernel answering 90 ms after launch; control round trip 1577 us; state in /home/me/.rhizo
$ rhizo run python-agent --llm http://127.0.0.1:11434 --model qwen2.5:1.5b
[rhizo] cell 1: CPython with `agent_loop.py`, model `qwen2.5:1.5b` at http://10.0.2.2:11434 as the guest sees it; started in 22.8 ms (round trip 36.0 ms)
[agent] user: Explore the sums and keep notes.
[turn 1] model (8.5 ms): I think the sums look fine.
[turn 1] no tool call: asked again (python 5.7 ms)
[rhizo] suspended between turns (15431 KiB snapshot, connection parked) in 24.8 ms (round trip 32.8 ms), resumed in 3235 us (round trip 10.2 ms)
[turn 2] model (11.6 ms): calls compute(n=2100, seed=3)
[turn 2] tool: value=9450, 1 notes (python and tool 56.6 ms)
...
[agent] decision point: asking for a fork
[rhizo] forked the agent at its decision point into 3 branches (cells [5, 6, 7], copy-on-write, own connections) in 8454 us (round trip 17.3 ms)
[branch 1] running
[branch 2] explores variant 2 of the last result: value=6968
[rhizo] all 3 branches running 106.8 ms after the fork request
...
[branch 0 turn] tool: value=10800, 4 notes (python and tool 3.4 ms)
rhizo-demo done in 1238 ms: 3 suspend and resume (median 28.0 ms), fork into 3 branches 8454 us (kernel times)
$ rhizo ps
ID     STATE                  IMAGE
1      killed                 cpython
5      exited 0               cpython
6      exited 0               cpython
7      exited 0               cpython
(4 cells, listed in 187 us (round trip 975 us))
$ rhizo down
```

(Under emulation, against a stand-in model that answers at once.)

`[agent]`, `[turn N]` and `[branch N]` lines come from the agent in the
cell (model time: request sent until the whole answer is read; Python time:
the rest of the turn). `[rhizo]` lines come from `rhizo`: the first time is
the kernel's own for the operation, the round trip is what the host saw.
Model answers are shown whole, on one line.

`cargo xtask demo agent --llm URL --model NAME [--vmm V]` does the same in
one command: it builds the control kernel and `rhizo`, then runs `rhizo up`,
`rhizo run python-agent` and `rhizo down`.

This is a demo: CI runs the same agent through `rhizo` against a
deterministic fake model instead (`cargo xtask test --only control`, and the
`workload.agent_*` and `control.*` metrics in `bench/README.md`), so no
model is downloaded there.

## What you need

- Everything in the README's "Try it" section, plus `zlib1g-dev`.
- CPython's pinned source, built into the cell on first use (a few
  minutes, then cached under `target/cpython`). It is built natively, so
  the kernel's architecture is the host's (`rhizo up` picks it).

```bash
curl -LO https://www.python.org/ftp/python/3.13.18/Python-3.13.18.tar.xz
export RHIZO_CPYTHON_SRC=$PWD/Python-3.13.18.tar.xz
```

- `rhizo` itself, from the checkout:

```bash
cargo build -p rhizo-cli
alias rhizo=$PWD/target/debug/rhizo
```

- A model that can call tools. Small ones work: Qwen2.5 1.5B Instruct,
  Llama 3.2 3B Instruct. When a model answers without calling the tool
  (small ones often do), the turn still completes: its reply is shown and
  the agent asks again.

## With Ollama

```bash
ollama serve &
ollama pull qwen2.5:1.5b
rhizo up
rhizo run python-agent --llm http://127.0.0.1:11434 --model qwen2.5:1.5b
rhizo down
```

## With llama.cpp

`--jinja` makes `llama-server` apply the model's chat template, which is
what turns its output into tool calls:

```bash
llama-server -m qwen2.5-1.5b-instruct-q4_k_m.gguf --jinja --port 8080 &
rhizo up
rhizo run python-agent --llm http://127.0.0.1:8080 --model qwen2.5
rhizo down
```

(`llama-server` serves the one model it loaded, whatever `--model` says.)

## Talk to an agent

`rhizo chat` puts you in a conversation with an agent running in a cell:
CPython with `cells/python/agent_chat.py`, which keeps the whole
conversation in the cell and gives the model four tools that run inside
it: a Python snippet (its output capped at 4000 characters, stopped after
10 s), read a file, write a file, list a directory. Type a message, read
the answer, type the next; after each answer `rhizo` prints the turn's
time, the model's apart from Rhizo's.

```text
$ rhizo up
$ rhizo chat --llm http://127.0.0.1:11434 --model qwen2.5:1.5b
[rhizo] chat cell 1: CPython with `agent_chat.py`, model `qwen2.5:1.5b` at http://10.0.2.2:11434 as the guest sees it; started in 3481 us (round trip 10.4 ms)
[rhizo] ready 586.2 ms after the request; type a message, /help for commands
[1]> How many primes are there below 10000?
  [tool] run_python
      print(sum(all(n % d for d in range(2, int(n**0.5) + 1)) for n in range(2, 10000)))
  [tool] ->
      1229
There are 1229 primes below 10000.
[rhizo] model 2950.4 ms, rhizo 61.2 ms (agent in the cell 52.0 ms, delivery 9208 us)
[1]> /fork 2
[rhizo] forked cell 1 into [2, 3] (copy-on-write, the conversation shared) in 3949 us (round trip 6207 us); still on 1
[1]> /switch 3
[rhizo] now talking to cell 3
[3]> Write them to /primes.txt instead.
...
[3]> /suspend
[rhizo] suspended cell 3 (17208 KiB snapshot) in 531.7 ms (round trip 538.2 ms); `rhizo chat 3` resumes it
$ rhizo chat 3
[rhizo] talking to cell 3, resumed in 15.3 ms (round trip 16.4 ms); /help for commands
[3]>
```

(An illustration under emulation: Rhizo's times are of the order the
`control` test step measures against the fake model, the model's depends
on yours. The benchmark's figure for a turn is `chat.turn_us`, see
`bench/README.md`.)

With Ollama or llama.cpp, as for the scripted demo (any model that calls
tools; small ones sometimes answer without calling them, which is fine):

```bash
ollama serve & ollama pull qwen2.5:1.5b
rhizo up
rhizo chat --llm http://127.0.0.1:11434 --model qwen2.5:1.5b

llama-server -m qwen2.5-1.5b-instruct-q4_k_m.gguf --jinja --port 8080 &
rhizo chat --llm http://127.0.0.1:8080 --model qwen2.5
```

Inside a chat:

| Type | What happens |
|---|---|
| a message | It goes to the agent's standard input; its answer, and each tool call with its result, is shown whole, then the time split |
| `/fork K` | Forks the conversation into `K` new cells sharing its memory and history copy-on-write; you stay on the current one |
| `/branches` | The branches this session knows (the first cell and those it forked or switched to), with their states |
| `/switch N` | Talk to cell `N` from now on (resumed first if suspended) |
| `/suspend` | Suspends the current branch and leaves; `rhizo chat N` resumes it later, history and files included |
| `/exit` | Ends the current branch and leaves (other branches keep running: `rhizo ps`, `rhizo kill N`) |
| `/help` | The commands |
| Ctrl-D | Closes the agent's input: it ends, and the chat with it |

`model` is the time the agent waited for the model's answers (requests
sent until answers read); `rhizo` is everything else from Enter to the
answer's last line: the agent's own work in the cell (JSON, tools,
printing) and the delivery both ways (the kernel and the serial line).

How it works: a cell started for a chat reads its standard input from a
pipe whose write end the host holds; each line is a control request
(`input`), and Ctrl-D closes the host's end (`close-input`). The pipe is
part of the cell, so a suspended branch keeps unread input, and each fork
gets its own copy; `rhizo` decides which branch you are talking to. With
standard input from a file or a pipe, `rhizo chat` echoes each line
(`> ...`) instead of prompting, so a conversation can be scripted: CI does
exactly that against the fake model (`cargo xtask test --only control`).

Limits: one conversation on screen; answers appear whole, not token by
token; a turn cannot be interrupted (a snippet stops by itself after 10 s,
but a single long call into C runs to its end); a new `rhizo chat N` knows
only cell `N` until you fork or switch.

## Driving cells by hand

`rhizo up` builds the control kernel on first use (or takes `--kernel`) and
leaves a small background server that owns the VM; its state (the control
socket, the kernel's console in `console.log`) is in `~/.rhizo`, or
`--state DIR`, or `$RHIZO_STATE`. Then:

```bash
rhizo run hello                       # a program the kernel holds, to its end
rhizo run idle --detach               # prints `cell N started`
rhizo suspend N                       # snapshot kept in the kernel
rhizo resume N                        # same identifier
rhizo fork N 3                        # three copy-on-write children
rhizo attach N                        # its output until it ends
rhizo kill N                          # a second kill forgets it
rhizo run python --file /app/x.py='print(6*7)' -- /app/x.py
rhizo ps
rhizo stats
```

Every command prints the kernel's time and the round trip.

## Which VMM, and where the guest finds the model

`rhizo up --vmm` picks the virtual machine (default: QEMU `microvm` on
x86_64, QEMU `virt` on AArch64); `--memory`, `--cpus` and `--accel` set the
rest. Give `--llm` as the host sees the model; `rhizo` rewrites a loopback
address to the host's address as the guest sees it:

| VMM | Guest network | The host, seen from the guest | The model must listen on |
|---|---|---|---|
| `qemu-microvm`, `qemu-q35`, `qemu-virt` | QEMU user network (DHCP) | `10.0.2.2` | `127.0.0.1` is enough |
| `firecracker` | tap device `rhizotap0` | `172.16.0.1` | `172.16.0.1` or `0.0.0.0` |
| `cloud-hypervisor` | tap device `rhizotap1` | `172.16.1.1` | `172.16.1.1` or `0.0.0.0` |

For Firecracker and Cloud Hypervisor, make the model listen beyond the
loopback: `OLLAMA_HOST=0.0.0.0 ollama serve`, or `llama-server --host
0.0.0.0`. Both need KVM, and the tap devices need password-less `sudo`
(`rhizo up` creates them). Another IPv4 address in `--llm` is passed as it
is: QEMU's user network reaches other machines; the tap devices have no
route beyond the host.

QEMU uses KVM when `/dev/kvm` is usable; `--accel tcg` forces emulation.

## What to expect

The model's time dominates every turn: from a few hundred milliseconds to
several seconds per answer for a 1.5B model on a CPU, more for longer
conversations. Rhizo's own steps, measured by the kernel:

| Step | With KVM (CI's figures, fake model) | Under emulation (TCG) |
|---|---|---|
| Python and tool, per turn | about 0.1 ms | 5 to 100 ms |
| Suspend (15 MiB snapshot) plus resume | about 2.7 ms (eager resume) | 25 to 55 ms (lazy resume) |
| Fork into 3 branches | about 1.2 ms | 10 to 80 ms |
| A request's round trip from the host | `control.roundtrip_us` | 0.3 to 3 ms |

Under emulation everything in the guest is 10 to 100 times slower, but the
model runs natively on the host, so the demo stays usable.

## Caveats

- **Plain HTTP only.** The cell's CPython has no TLS yet (design D2 of
  `add-agent-workload`), so hosted APIs over HTTPS are out of reach; run
  the model locally, or put a plain-HTTP proxy next to it.
- **IPv4 addresses**, not host names, in `--llm`.
- **The guest polls.** Its network card and its serial line (the control
  channel) are polled: a turn's model time includes up to one polling
  period, and an idle control kernel wakes every 2 ms (design D4 of
  `add-control-plane`).
- **The serial line is the control channel** on every VMM (design D2 of
  `add-control-plane`): fine for requests and output, slow for bulk data.
- **Snapshots stay in the VM's memory**; `rhizo down` drops them.
- **Memory.** `rhizo up` gives the guest 256 MiB: the agent and its three
  branches fit easily (they share their pages copy-on-write).
- **Linux hosts only.**
