# A real agent loop in CPython (spec `workloads`, change
# `add-agent-workload`): it talks to an OpenAI-compatible chat-completions
# API (`/v1/chat/completions` with tools: the host's fake model in CI;
# Ollama or llama.cpp's `llama-server` in a live demo) over HTTP/1.1 on one
# kept-alive TCP connection, and turn after turn sends the conversation,
# parses the JSON answer, runs the tool the model asks for (a small
# computation plus a file written and read back) and appends the result. At
# a decision point it forks into branches that each explore a different
# tool result.
#
#     python3 agent_loop.py URL MODEL MODE TURNS [timed|live]
#
# URL is the API's base (`http://HOST:PORT`, optionally ending in `/v1`),
# MODEL the model's name in each request. MODE says how the branches are
# made:
# - `cell`: Rhizo forks the cell. The agent prints a line ending in `fork`
#   and waits for the file `/branch`, looking every few milliseconds; the
#   host forks it there (a template it starts cells from, or a fork through
#   the control plane) and gives each branch its number in `/branch`, in
#   that branch's own files. Between turns the
#   agent pauses briefly, and the host may suspend and resume it there: its
#   connection to the model survives (the model's request count on it goes
#   on).
# - `linux`: the same agent as a Linux process: `os.fork` per branch, each
#   branch's output sent back through a pipe and printed in branch order.
#
# The transcript is the same in both modes and on every run with the fake
# model. With `timed`, the agent also prints `agent turn_ns=...` (each
# turn: request, answer, tool, append), `agent model_ns=...` (each turn's
# request sent until its answer is read) and, in `linux` mode,
# `agent fork_ns=...` (from the first `os.fork` until every branch runs; in
# `cell` mode the host measures it). With `live`, it prints a readable
# account instead of the transcript: the prompt, each answer and tool call,
# and each step's time on the model and in Python. A model that answers
# without calling the tool (small ones do) is asked to call it again.
# Without arguments it runs one turn offline (how its standard library is
# packed, cells/python/pack_stdlib.py).
import hashlib
import json
import os
import socket
import sys
import time

BRANCHES = 3
PAUSE = 0.001
NOTES = "agent-notes.txt"
PROMPT = "Explore the sums and keep notes."
TOOLS = [
    {
        "type": "function",
        "function": {
            "name": "compute",
            "description": "Sum of i*i mod (seed+7) for i below n; notes it in a file.",
            "parameters": {
                "type": "object",
                "properties": {"n": {"type": "integer"}, "seed": {"type": "integer"}},
            },
        },
    }
]
LIVE = False


def say(text):
    """A line of the live account."""
    if LIVE:
        print(text, flush=True)


def ms(ns):
    return "%.1f ms" % (ns / 1e6)


class Model:
    """A chat-completions client: HTTP/1.1 kept alive on one connection."""

    def __init__(self, url, name):
        rest = url.split("://", 1)[-1]
        hostport, _, path = rest.partition("/")
        host, _, port = hostport.partition(":")
        path = "/" + path.strip("/")
        if not path.endswith("/v1"):
            path = path.rstrip("/") + "/v1"
        self.addr = (host, int(port or 80))
        self.path = path + "/chat/completions"
        self.name = name
        self.sock = None
        self.buf = b""
        self.waited = []

    def connect(self):
        if self.sock is not None:
            self.sock.close()
        # As HTTP clients do: name resolution (the `idna` codec), no
        # timeout (the default of `requests` and `http.client`: a real model
        # on a CPU may think for minutes; the kernel bounds the run), and
        # `TCP_NODELAY` once connected.
        self.sock = socket.create_connection(self.addr)
        self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self.buf = b""

    def recv(self):
        data = self.sock.recv(65536)
        if not data:
            raise ConnectionError("the model closed the connection")
        self.buf += data

    def line(self):
        while b"\r\n" not in self.buf:
            self.recv()
        line, _, self.buf = self.buf.partition(b"\r\n")
        return line

    def take(self, n):
        while len(self.buf) < n:
            self.recv()
        data, self.buf = self.buf[:n], self.buf[n:]
        return data

    def chat(self, messages):
        body = json.dumps({"model": self.name, "messages": messages, "tools": TOOLS}).encode()
        head = (
            "POST %s HTTP/1.1\r\nHost: %s:%d\r\n"
            "Content-Type: application/json\r\nContent-Length: %d\r\n\r\n"
            % (self.path, self.addr[0], self.addr[1], len(body))
        )
        t0 = time.perf_counter_ns()
        self.sock.sendall(head.encode() + body)
        status = self.line().decode()
        headers = {}
        while line := self.line():
            name, _, value = line.decode().partition(":")
            headers[name.strip().lower()] = value.strip()
        if headers.get("transfer-encoding", "").lower() == "chunked":
            answer = b""
            while size := int(self.line().split(b";")[0], 16):
                answer += self.take(size)
                self.line()
            while self.line():  # trailers, then the empty line
                pass
        else:
            answer = self.take(int(headers.get("content-length", "0")))
        self.waited.append(time.perf_counter_ns() - t0)
        if status.split()[1:2] != ["200"]:
            raise RuntimeError("%s: %s" % (status, answer[:300].decode(errors="replace")))
        return json.loads(answer)


class Offline:
    """A canned model, for packing the standard library."""

    waited = [0]

    def chat(self, messages):
        call = {"id": "call_0", "function": {"name": "compute", "arguments": '{"n": 9, "seed": 1}'}}
        return {"choices": [{"message": {"tool_calls": [call]}}], "x_requests_on_connection": 1}


def compute(n, seed):
    """The tool: a small computation, noted in a file that is read back."""
    value = sum(i * i % (seed + 7) for i in range(n))
    with open(NOTES, "ab") as f:
        f.write(b"n=%d seed=%d value=%d\n" % (n, seed, value))
    with open(NOTES, "rb") as f:
        notes = f.read()
    return {"value": value, "notes": notes.count(b"\n"), "sha": hashlib.sha256(notes).hexdigest()[:8]}


def arguments(call):
    """The tool call's `n` and `seed`, as integers in bounds (a real model
    may give strings, other keys or no arguments at all)."""
    raw = call.get("function", {}).get("arguments") or "{}"
    try:
        args = raw if isinstance(raw, dict) else json.loads(raw)
    except ValueError:
        args = {}

    def number(key, default, top):
        try:
            return max(0, min(int(args.get(key, default)), top))
        except (TypeError, ValueError):
            return default

    return number("n", 100, 200000), number("seed", 0, 1000000)


def turn(model, messages, label):
    """One turn: send the conversation, run the tool it asks for, append."""
    t0 = time.perf_counter_ns()
    answer = model.chat(messages)
    waited = model.waited[-1]
    message = answer["choices"][0]["message"]
    requests = answer.get("x_requests_on_connection", 0)
    calls = message.get("tool_calls") or []
    if not calls:
        text = message.get("content") or ""
        messages.append({"role": "assistant", "content": text})
        messages.append({"role": "user", "content": "Call the compute tool with other numbers."})
        say("[%s] model (%s): %s" % (label, ms(waited), " ".join(text.split()) or "(empty)"))
        say("[%s] no tool call: asked again (python %s)" % (label, ms(time.perf_counter_ns() - t0 - waited)))
        return "%s reply chars=%d conn_requests=%d" % (label, len(text), requests)
    call = calls[0]
    n, seed = arguments(call)
    say("[%s] model (%s): calls compute(n=%d, seed=%d)" % (label, ms(waited), n, seed))
    result = compute(n, seed)
    messages.append({"role": "assistant", "content": None, "tool_calls": [call]})
    messages.append({"role": "tool", "tool_call_id": call.get("id", "call"), "content": json.dumps(result)})
    say(
        "[%s] tool: value=%d, %d notes (python and tool %s)"
        % (label, result["value"], result["notes"], ms(time.perf_counter_ns() - t0 - waited))
    )
    return "%s %s n=%d seed=%d value=%d notes=%d sha=%s conn_requests=%d" % (
        label,
        call.get("id", "call"),
        n,
        seed,
        result["value"],
        result["notes"],
        result["sha"],
        requests,
    )


def branch(model, messages, b):
    """A branch: the last tool result explored with variant `b`, with notes
    and a connection of its own, then one more turn."""
    global NOTES
    try:
        with open(NOTES, "rb") as f:
            notes = f.read()
    except FileNotFoundError:  # the model never called the tool
        notes = b""
    NOTES = "agent-notes-%d.txt" % b
    with open(NOTES, "wb") as f:
        f.write(notes)
    if messages[-1]["role"] == "tool":
        call = messages[-2]["tool_calls"][0]
        n, seed = arguments(call)
        result = compute(n, seed + b)
        messages[-1] = {"role": "tool", "tool_call_id": call.get("id", "call"), "content": json.dumps(result)}
    else:
        result = compute(100, b)
        messages.append({"role": "user", "content": "Variant %d gave %s." % (b, json.dumps(result))})
    say("[branch %d] explores variant %d of the last result: value=%d" % (b, b, result["value"]))
    model.connect()
    return "branch %d value=%d\n%s\n" % (b, result["value"], turn(model, messages, "branch %d turn" % b))


def fork_linux(model, messages, timed):
    """`linux` mode: one process per branch, outputs printed in order."""
    t0 = time.perf_counter_ns()
    pipes = []
    for b in range(BRANCHES):
        r, w = os.pipe()
        if os.fork() == 0:
            os.close(r)
            os.write(w, b"branch %d\n" % b)
            os.write(w, branch(model, messages, b).encode())
            os._exit(0)
        os.close(w)
        pipes.append(r)
    outputs = []
    for r in pipes:
        data = b""
        while b"\n" not in data:
            data += os.read(r, 4096)
        outputs.append(data)
    ran = time.perf_counter_ns() - t0
    for r, data in zip(pipes, outputs):
        while more := os.read(r, 4096):
            data += more
        os.close(r)
        sys.stdout.write(data.decode())
    for _ in pipes:
        if os.wait()[1] != 0:
            sys.exit("a branch failed")
    if timed:
        print("agent fork_ns=%d" % ran)


def run(model, mode, turns, timed):
    model.connect()
    messages = [
        {"role": "system", "content": "You are a careful agent. Use the tools."},
        {"role": "user", "content": PROMPT},
    ]
    say("[agent] user: %s" % PROMPT)
    spent = []
    for t in range(1, turns + 1):
        t0 = time.perf_counter_ns()
        line = turn(model, messages, "turn %d" % t)
        spent.append(time.perf_counter_ns() - t0)
        if not LIVE:
            print(line, flush=True)
        time.sleep(PAUSE)  # between turns: the host may suspend the cell here
    if timed:
        print("agent turn_ns=%s" % ",".join(map(str, spent)), flush=True)
        print("agent model_ns=%s" % ",".join(map(str, model.waited)), flush=True)
    print("[agent] decision point: asking for a fork" if LIVE else "fork", flush=True)
    if mode == "linux":
        fork_linux(model, messages, timed)
        return
    while not os.path.exists("/branch"):  # the host forks the cell here
        time.sleep(0.01)
    with open("/branch", "rb") as f:
        b = int(f.read())
    print("[branch %d] running" % b if LIVE else "branch %d" % b, flush=True)
    lines = branch(model, messages, b)
    if not LIVE:
        sys.stdout.write(lines)


def main():
    global LIVE
    if len(sys.argv) < 5:
        "model".encode("idna")  # what `getaddrinfo` does with a host name
        print(turn(Offline(), [], "offline"))
        return
    url, name, mode, turns = sys.argv[1:5]
    LIVE = sys.argv[5:] == ["live"]
    run(Model(url, name), mode, int(turns), sys.argv[5:] == ["timed"])


if __name__ == "__main__":
    main()
