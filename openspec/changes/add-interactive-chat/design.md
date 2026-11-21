# Design

## D1. Standard input: a pipe whose write end the host holds

A cell started with `Run::input` gets, on descriptor 0, the read end of a
new pipe of the cell's file system (`rhizo-fs`, 64 KiB like every pipe).
Its one write end is not a descriptor: the file system records the pipe's
object index (`Tables::input`) and counts the host as its writer. Nothing
else is new: reads, `poll`, `select`, `epoll`, `dup`, `close` and
non-blocking mode behave as for any pipe, so CPython's buffered `stdin`
works unchanged.

- `Input { cell, data }` writes `data` whole or not at all: refused when
  the cell has no host input or the host closed it, when the cell closed
  every read end, or when the unread bytes leave less room than `data`
  (the host retries later). The bytes live in the kernel heap and count
  against the memory cells may take, as a cell's own pipe writes do.
- `CloseInput { cell }` drops the host's end: once the rest is read,
  reads return 0 (end of file). Input after it is refused.
- **Suspend and resume.** The pipe is part of the cell's files, so its
  snapshot carries the unread bytes and whether the host's end is open
  (the file system's encoding gains the index, snapshot format version
  7); the resumed cell reads on as if nothing happened, and the host
  writes again. Input to a suspended cell is refused (`NotRunning`):
  `rhizo` resumes it first.
- **Fork and templates.** A fork's children copy the cell's file system
  (copy-on-write tables), pipe included: each branch has its own standard
  input holding the bytes unread at the fork, and an open host end of its
  own. A cell spawned from a template does the same. Which branch
  receives what is the host's choice; the kernel never broadcasts.
- **Without `input`** descriptor 0 stays the console: reads give end of
  file, as before. No other cell changes.

Alternatives that lost: a host-held descriptor in the cell (the cell could
close or `dup2` over it, and fork would share one description between
branches); a new descriptor kind for "host input" (a second pipe
implementation, and every `poll` and `epoll` path would need it); giving
every control-plane cell a pipe (programs that read standard input to its
end, such as `python` without a script, would wait forever instead of
seeing end of file).

## D2. Protocol: version 2, two requests, bounds

- Kinds 0x0b `input` (`cell: u32`, `data: bytes`) and 0x0c `close-input`
  (`cell: u32`); `run` gains a last byte, `input` (0 or 1, anything else
  malformed). The `run` payload changed shape, so the version goes to 2: a
  `rhizo` of version 1 is refused with "unsupported protocol version"
  instead of misread.
- **Bounds.** A frame is already at most 1 MiB, and the decoder reserves
  its payload with `try_reserve`. An `input` request holds at most
  `MAX_INPUT` = 64 KiB (refused as `TooLarge` above), equal to the pipe's
  capacity, so an accepted request fits an empty pipe; the pipe never holds
  more than 64 KiB per cell, and only cells the host started with `input`
  have one. So a host can make the kernel hold at most 64 KiB per such
  cell through input, and those bytes count against cells' memory.
- **Fuzzing.** The property tests that run in `cargo xtask check`
  round-trip the new requests (with arbitrary data and flags) between
  console text, feed arbitrary bytes and payloads under every kind, damage
  frames, and check that any `input` payload decodes within its bound or is
  refused; the file system's decoder refuses an input index that names no
  pipe, or a pipe whose writer count does not include the host.

## D3. Waking the reader, and getting the answer out promptly

`Host::input` takes the cell out of the scheduler (no cell runs while the
control kernel handles a request), writes or closes, marks its threads that
wait on descriptors runnable (`wake_io`: they make their `read` or `poll`
again, design D3 of `add-sockets-and-poll`) and files it back. A thread
blocked on fd 0 thus runs at the next scheduling step.

The control kernel sent a watched cell's output every 10 ms. A chat turn
ends with the agent printing its answer and blocking on its next read: the
kernel now also stops its run loop when a watched cell has output waiting
and none of its threads can run (`Host::has_output`), so the answer goes
out at once instead of up to 10 ms later. A cell that prints while it
computes is still flushed every 10 ms. Measured with `chat.turn_us` on
QEMU `microvm` under TCG (one run of 20 turns each): 15.6 ms median with
the check, 23.8 ms without it.

## D4. The chat agent

`cells/python/agent_chat.py` (one file next to `agent_loop.py`, whose
`Model` HTTP/1.1 client it imports; `Model.chat` now takes the tool list)
reads one message per line from `sys.stdin`, appends it to the conversation
and calls the model until it answers without tool calls (at most 8 requests
per message). The conversation is a Python list in the cell: fork and
suspend carry it with the rest of the cell's memory.

Tools, all inside the cell: `run_python(code)` (its own namespace, output
captured and capped at 4000 characters, stopped by a line trace after 10 s:
cells have no signals, so `SIGALRM` cannot interrupt it; a single long call
into C is not interrupted), `read_file(path)` (16000 characters),
`write_file(path, content)` and `list_dir(path)`. Errors are results the
model reads. A request that fails on a dead connection (closed by the
model's server, reset by a fork, `add-control-plane` D6 for suspend) is
retried once on a new connection; a model that fails outright gets
`[agent] the model failed: ...` and the message is dropped from the
history.

What it prints is the transcript the user reads, plus two marker lines
starting with 0x1e (never shown): `\x1eready` once it reads its input, and
`\x1eturn model_ns=M agent_ns=A` after each answer (the model's time:
each request sent until its answer is read; the agent's: from the line read
to the marker). Tool calls are shown as `  [tool] NAME`, their arguments and
result indented below; answers are shown whole.

Alternatives that lost: `agent_loop.py --chat` (its fixed script, `live`
mode and branch logic would interleave with the chat's, and its CI
transcript must stay byte for byte); streaming tokens (`"stream": true`
and server-sent events: more code in the cell for a demo whose model is
local; the whole answer appears at once).

## D5. `rhizo chat`

- `rhizo chat --llm URL --model NAME` runs `cpython` with `cpython-files`,
  `agent_chat.py URL MODEL` (the loopback rewritten as for
  `python-agent`) and `input`, streams its output and waits for
  `\x1eready`. `rhizo chat N` lists the cells, resumes `N` if suspended,
  refuses an ended one, and streams it.
- Each line read from the terminal (plain `read_line`, no line-editing
  library: none is needed for the scope, and none is small enough to be
  worth a dependency) is either a command or a message, sent as `Input`
  with a newline. The answer's lines are printed as they arrive; the turn
  marker ends the turn and prints
  `[rhizo] model X, rhizo Y (agent in the cell Z, delivery W)`: `Y` is
  the host's time from the request to the marker, less the model's; `Z` is
  the agent's own time less the model's; `W` the rest (kernel, serial
  line, host). The next line is read only then.
- `/fork K` forks the current branch into `K` (1 to 64) and stays;
  `/branches` lists the branches this session knows (the first cell and
  the ones it forked or switched to) with their states from `list`;
  `/switch N` resumes `N` if needed, streams it and sends input there;
  `/suspend` suspends the current branch and leaves; `/exit` kills it and
  leaves (naming branches that may still run); `/help`. Ctrl-D (end of the
  terminal's input) sends `CloseInput`: the agent ends, and the chat with
  it.
- When standard input is not a terminal (a pipe or a file), there is no
  prompt and each line is echoed as `> LINE`, so a scripted conversation's
  output reads like a typed one. Lines of other branches, should any
  print, are prefixed with their cell.

The kernel keeps no family tree, so `rhizo chat N` knows only `N` until
its user forks or switches; `rhizo ps` lists every cell.

## D6. Tests and the metric

- Host: `rhizo-fs` (input, whole-or-nothing, end of file, a cell that
  closed its input, fork copies, snapshot round trip, corrupt index),
  `rhizo-control` (round trips, oversized and malformed input, the flag,
  property tests), `rhizo` (arguments, chat lines, the turn line), `xtask`
  (marker lines, the fake chat model).
- `control` step (both architectures, QEMU, with CPython): `rhizo chat`
  driven through a pipe by the test as a user would type, each step read
  up to its `[rhizo]` line: two messages, `/fork 2`, `/branches` (three
  running), `/switch` to a branch, a message, `/suspend`, `rhizo chat N`
  (resumed), a message, end of input (the agent exits 0); the agent's
  lines and the echoed messages equal `CHAT_TRANSCRIPT`, and every turn
  printed its time split.
- `chat.turn_us` (`cargo xtask bench`, per VMM, when CPython is built):
  20 turns of the chat agent against the fake model with no delay, each
  from the host writing the `input` request to the VMM's standard input
  until it reads the agent's turn marker back, less the model time the
  agent measured; median and p90. It is the Rhizo side of a turn: request
  delivery, waking the reader, CPython's work (JSON, the tool's snippet,
  printing), the answer's output and the serial line. Not gated until a
  CI run sets its baseline, like the other control-plane metrics.
