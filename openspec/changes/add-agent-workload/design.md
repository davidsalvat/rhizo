# Design

## D1. The fake model: a thread of `xtask` next to the echo server

`hostnet::llm_server(addr)` listens on the host side of each guest network
(127.0.0.1 for QEMU's user network, seen as 10.0.2.2; the tap device's
address for Firecracker and Cloud Hypervisor) and every Rhizo guest with a
network gets `llm=ADDR:PORT` on its command line, next to `echo=`. One
thread per connection reads HTTP/1.1 requests (headers, then
`Content-Length` bytes), waits `RHIZO_LLM_DELAY_MS` (0 by default: the
benchmark measures Rhizo, not a sleep), and answers a chat completion that
calls the `compute` tool. The tool's arguments and the call's identifier
are an FNV-1a hash of the request body, so the same conversation always
gets the same answer. The answer is formatted without parsing the request
as JSON: `xtask` runs as a debug build, and the server's own time is part
of every turn measured, on both sides (`serde_json` parsing a 3 KiB
conversation in a debug build cost more than the kernel's part of a turn).
The connection stays open unless the
request says `Connection: close` (so a client that reads to the end of the
stream works too, as the laptop workload of `add-reference-workloads`
does); each answer carries `x_requests_on_connection`, which lets the
agent's transcript show that its connection survived a suspend.

Alternatives: a Python server (one more process to manage, and xtask is
where the echo server lives), a real model (measures the model), a fixed
answer (a transcript could not show that the conversation reached the
model intact).

## D2. HTTP without TLS, through CPython's own sockets

The agent uses CPython's `socket` module (`_socket`, and `select` and
`array`, which `socket.py` imports), built into the static executable like
the other C modules, and connects as HTTP clients do:
`socket.create_connection` with a timeout. That resolves the host through
`getaddrinfo`, whose host name CPython encodes with the `idna` codec
(`unicodedata`), and puts the socket in non-blocking mode with
`ioctl(FIONBIO)`, which the kernel now implements with `FIOCLEX` and
`FIONCLEX` (`os.set_inheritable`) instead of refusing every `ioctl` with
`ENOTTY`. Its tool hashes its notes with `hashlib`, so the hash modules
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

- `workload.agent_turn_us`: one turn as the agent itself times it with
  `time.perf_counter_ns` (request, answer, JSON, tool, append), the same
  code on both sides; median of turns 2 to 8 (the first connects and runs
  cold code) of runs that are not suspended, as the Linux process is not
  (a turn right after a resume runs with cold caches and TLB).
- `workload.agent_model_us`: the part of those turns spent on the model,
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
