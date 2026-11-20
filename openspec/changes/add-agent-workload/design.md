# Design

## D1. The fake model: `add-reference-workloads`' server, kept alive and deterministic

The laptop workload of `add-reference-workloads` brought a fake model
server to `xtask` (`hostnet::llm_server`, `llm=ADDR:PORT` on the command
line). This change reuses it rather than adding a second one, and extends
it in three compatible ways:

- **Every guest network gets it**: `for_vmm` adds `llm=` (no delay) next
  to `echo=`, as the agent test and benchmark run on every VMM with a card.
  The laptop workload still passes its own `llm=` (with its
  `--llm-delay-ms`), later on the command line, which wins.
- **Kept-alive connections**: one thread per connection answers request
  after request until the client closes it or sends `Connection: close`.
  The laptop's `agent` cell sends `Connection: close` and reads to the end
  of the stream, so it sees what it saw before. Each answer carries
  `x_requests_on_connection`, which lets the agent's transcript show that
  its connection survived every suspend.
- **Deterministic tool calls**: the answer calls the `compute` tool with
  arguments and an identifier from an FNV-1a hash of the request body, so
  the same conversation always gets the same answer and a transcript can
  be recorded. The answer is formatted without parsing the request as
  JSON: `xtask` is a debug build and the server's time is part of every
  turn measured, on both sides (`serde_json` parsing a 3 KiB conversation
  in a debug build cost more than the kernel's share of a turn). The
  laptop's fixed answer (`LLM_ANSWER`) goes, and its test with it: the
  kept-alive test replaces it.

Alternatives: a second server for the CPython agent (two fake models to
keep in step), a Python server (one more process to manage), a real model
in CI (a download, and it measures the model).

## D2. HTTP without TLS, through CPython's own sockets

The agent uses CPython's `socket` module (`_socket`, and `select` and
`array`, which `socket.py` imports), built into the static executable like
the other C modules, and connects as HTTP clients do:
`socket.create_connection`, which resolves the host through `getaddrinfo`,
whose host name CPython encodes with the `idna` codec (`unicodedata`). A
socket with a timeout is put in non-blocking mode with `ioctl(FIONBIO)`,
which the kernel now implements with `FIOCLEX` and `FIONCLEX`
(`os.set_inheritable`) instead of refusing every `ioctl` with `ENOTTY`.
The agent itself uses no timeout, the default of `requests` and
`http.client` (a real model on a CPU may think for minutes; the kernel
bounds the run). Its tool hashes its notes with `hashlib`, so the hash modules
`hashlib` imports are built in too (`_md5`, `_sha1`, `_sha2`, `_sha3`,
`_blake2`: HACL* and BLAKE2 code in CPython's tree, no OpenSSL); without
them `hashlib` prints an error for every algorithm at import. Its HTTP/1.1 client is twenty lines in the script
(request, headers, `Content-Length` body, kept alive): `http.client` would
work too but imports the `email` package (some thirty more modules to pack)
for header parsing the agent does not need.

**TLS is left for a follow-up.** HTTPS needs OpenSSL 3.x (Apache-2.0)
built statically into CPython's `_ssl` and `_hashlib` for two
architectures in CI (a several-minute build per architecture, cached like
CPython, and a few MiB more in the kernel image, which embeds CPython for
its tests), a self-signed certificate the host server and the agent trust,
and cryptographic `getrandom`, which the parallel `add-crypto-random`
change brings. None of it changes what this change proves: TLS state is
ordinary cell memory, so it survives a suspend like the connection under
it, and each forked branch opens its own connection (D3), so it would make
its own handshake. Plain HTTP to the host keeps the turn measuring the
kernel paths the comparison is about.

## D3. Fork is the host's: a template at the decision point

Cells have no processes (`clone` without `CLONE_THREAD` is refused, design
D5 of `add-threads`): Rhizo's fork is an operation of the host on a whole
cell (spec `fork`), not a system call. So the agent's `os.fork` is not what
makes branches in a cell. At its decision point the agent prints `fork` and
sleeps for an hour; the host waits until the cell sleeps
(`Host::asleep`: none of its threads can run, one waits for a deadline and
none for a descriptor), freezes it as a template and starts three cells
from it. Templates keep no time (spec `fork`), so each wakes at once; the
host first writes the branch number into each one's `/branch`
(`Host::write_file`, the cell's own copy-on-write files). This is the real
primitive: what an orchestrator does to explore several continuations of
one agent. The only difference in the script between Rhizo and Linux is
this point (`cell` or `linux` mode); in `linux` mode the agent calls
`os.fork` per branch and sends each branch's output back through a pipe.

Each branch closes the connection it inherited and opens its own (a forked
cell's established connections are reset, `keep-tcp-across-suspend`;
forked Linux processes would share one connection) and copies the notes
file to one of its own (Linux processes share the directory; cells do not
but the transcript must be the same), then explores its variant of the
last tool result and runs one more turn.

Alternatives: process fork inside a cell (a process model, `wait`, pids: a
milestone of its own); `Host::fork` of the live cell (the parent goes on,
and the children inherit its hour-long sleep, which only templates end);
branch numbers from cell identifiers (not deterministic across runs).

Suspend points work the same way: after each turn the agent sleeps a
millisecond (a user's think time, short), and the host suspends and
resumes it once `Host::asleep` holds, never in the middle of a request.

## D4. Metrics

`cell::agent` (kernel, `ktest` and `bench-cell` builds) drives one run:
spawn CPython with the agent, step until each pause, suspend and resume,
freeze at `fork`, start the branches, run them to their end; it returns
the transcript, each suspend-plus-resume time and the fork time. The
`bench-cell` kernel runs it three times with 8 turns suspended after
each, and three times without suspending it, and prints
`rhizo-agent turn_ns=... model_ns=... suspend_resume_ns=... fork_ns=...` (or
`rhizo-agent skipped: <why>`):

- `workload.agent_cpython_turn_us`: one turn as the agent itself times it with
  `time.perf_counter_ns` (request, answer, JSON, tool, append), the same
  code on both sides; median of turns 2 to 8 (the first connects and runs
  cold code) of runs that are not suspended, as the Linux process is not
  (a turn right after a resume runs with cold caches and TLB).
- `workload.agent_cpython_model_us`: the part of those turns spent on the model,
  from sending the request until the whole answer is read (network card,
  TCP stack, the host's server), so a gap in the turn can be told apart
  from a gap in the interpreter.
- `workload.agent_suspend_resume_us`: suspend plus **eager** resume
  between two turns, so no restoring is deferred to page faults the figure
  would not count (the in-kernel test resumes lazily, the default).
- `workload.agent_fork_us`: from freezing the agent at its decision point
  to all three branches having printed their first line.

The Linux side measures the same turn and model part, and its fork from the first
`os.fork` until every branch wrote its first line to its pipe.

## D5. The Linux process in the same VM; no CRIU figure

`bench/host/linux_cpython.c`, already the guest's `/init` for the CPython
comparison, gets the VM's network card on the Firecracker tap device (the
one Rhizo's Firecracker guest uses, at the same address) and the model's
address from its command line (`rhizo_ip=`, `rhizo_llm=`, which Linux
hands to `/init` as environment variables), brings `lo` and `eth0` up with
`ioctl`s and runs the agent in `linux` mode three times with 8 turns.

Suspend and resume of the Linux process would be CRIU with
`--tcp-established`, which needs root, CRIU inside the guest (it is not in
the initramfs and the reference kernel is not built for checkpointing) or
the agent on the host instead of in the VM (not the same VM any more), and
TCP repair on a connection to a server that lives in the dumping process's
own parent (`xtask`). The figure would compare a different thing on a
different machine, so it is not reported; the existing CRIU restore figure
(`snapshot.restore_ms/criu/host`) stays the reference for process
checkpointing.

## D6. AArch64

CPython is built natively on each CI runner, the AArch64 one included
(`ci-arm64`, `ubuntu-24.04-arm`), so no cross-build is needed: the agent
test runs on AArch64 QEMU `virt` (emulated: no KVM on GitHub's ARM
runners), functionally only. The emulated run is slow, so the agent driver
gives a run ten minutes before it fails it.

## D7. The same agent against a real model: `cargo xtask demo agent`

The agent speaks the OpenAI-compatible API that Ollama and llama.cpp's
`llama-server` both serve: its first two arguments are the API's base URL
and the model's name, so the fake model and a real one are interchangeable
(CI passes `fake`). What a real model needs that the fake one does not:
answers in chunks (`Transfer-Encoding: chunked`), tool arguments that are
strings, missing or out of range (clamped), and answers that call no tool
(small models do): that turn ends with the model's text, the agent asks
again, and a branch whose last answer had no tool call explores its variant
as a user message. The differential check runs the agent against a model
that only ever answers text, in chunks.

The demo is a benchmark kernel booted with `bench=agent-demo model=NAME
llm=IP:PORT` (one more reference workload, so no new build), which runs
`cell::agent` with three turns, a lazy resume after each, and the agent's
`live` output: readable lines with each answer's model time and the rest of
the turn's time, echoed to the console as the cell prints them, between the
host's own lines for each suspend, resume and the fork. `xtask demo agent`
maps a loopback `--llm` to the host's address as the guest sees it (10.0.2.2
on QEMU's user network, the tap device's host address on Firecracker and
Cloud Hypervisor) and waits up to 30 minutes without output, as a CPU-bound
model under an emulated guest can be slow. It is not run in CI: it needs a
model. `docs/demo.md` says how to run it with Ollama and `llama-server`.

The demo drives the cell from the host in the kernel, like the benchmark:
the agent asks for a fork by printing a line ending in `fork` and sleeping,
and the host suspends it whenever it sleeps between turns. A control plane
outside the kernel (a host API to suspend, fork and resume cells on
request) is a product change, not this proof of concept's.

## D8. Two fixes found by the agent

- **A 2 MiB page restored without a 2 MiB run of frames.** Since
  `add-large-image-pages`, CPython's code is mapped with 2 MiB pages; a
  snapshot keeps them as 2 MiB pages, and `Snapshot::restore_page` needed
  a free 2 MiB run to bring one back. After a few suspends memory is
  fragmented, the lazy resume of the agent could not restore its code and
  the cell died on an instruction fetch. A 2 MiB page now comes back as
  512 pages of 4 KiB when no run is free, as `KernelFrames::alloc_large`
  already promised its callers (host test in `rhizo-snapshot`); the code
  cache is synchronized page by page.
- **Closing connections outlive the test.** `smoltcp` keeps a closed
  connection until its peer acknowledges the close, so the agent's
  connections to the model were still on the card (two heap allocations)
  when the next in-kernel test counted the heap. The agent's test now waits
  until the card holds no connection (`cell::wire_busy`) before it ends.
- **The cell benchmarks ran out of memory.** Since the kernel embeds the
  dynamic programs' files (`add-dynamic-linking`) next to CPython's, a
  `bench-cell` kernel in 128 MiB had 106 MiB left: not enough for the
  50 MiB template of `cell.fork_touch_us` and its fully written fork, so
  the benchmark kernel panicked before printing any cell figure, on every
  VMM, and the gate, which does not fail on missing metrics, passed. The
  cell metrics are now measured in a 256 MiB VM.
- **`TCP_NODELAY` set after connecting was ignored on the card.** The
  kernel gave a connection through the network card its Nagle setting when
  it connected only; CPython (like `http.client`) sets `TCP_NODELAY` once
  connected, so a request larger than a segment waited for the host's
  delayed acknowledgment: in CI a turn took 2.4 ms on QEMU and 41 ms on
  Firecracker and Cloud Hypervisor instead of 0.2 ms. The option now
  applies at once (host test in `rhizo-net`).
