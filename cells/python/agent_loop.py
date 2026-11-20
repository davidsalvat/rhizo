# A real agent loop in CPython (spec `workloads`, change
# `add-agent-workload`): it talks to a chat-completions API (the host's fake
# model) over HTTP/1.1 on one kept-alive TCP connection, and turn after
# turn sends the conversation, parses the JSON answer, runs the tool the
# model asks for (a small computation plus a file written and read back)
# and appends the result. At a decision point it forks into branches that
# each explore a different tool result.
#
#     python3 agent_loop.py HOST PORT MODE TURNS [timed]
#
# MODE says how the branches are made:
# - `cell`: Rhizo forks the cell. The agent prints `fork` and sleeps; the
#   host freezes it there as a template and starts one cell per branch from
#   it (templates keep no time, so each wakes at once), giving each its
#   branch number in the file `/branch`. Between turns the agent pauses
#   briefly, and the host may suspend and resume it there: its connection
#   to the model survives (the model's request count on it goes on).
# - `linux`: the same agent as a Linux process: `os.fork` per branch, each
#   branch's output sent back through a pipe and printed in branch order.
#
# The transcript is the same in both modes and on every run. With `timed`,
# the agent also prints `agent turn_ns=...` (each turn: request, answer,
# tool, append), `agent model_ns=...` (each turn's request sent until its
# answer is read) and, in `linux` mode, `agent fork_ns=...` (from the first
# `os.fork` until every branch runs; in `cell` mode the host measures it).
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


class Model:
    """A chat-completions client: HTTP/1.1 kept alive on one connection."""

    def __init__(self, host, port):
        self.addr = (host, port)
        self.sock = None
        self.buf = b""
        self.waited = []

    def connect(self):
        if self.sock is not None:
            self.sock.close()
        # As HTTP clients do: name resolution (the `idna` codec), and a
        # timeout, which puts the socket in non-blocking mode (`FIONBIO`).
        self.sock = socket.create_connection(self.addr, timeout=60)
        self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self.buf = b""

    def recv(self):
        data = self.sock.recv(65536)
        if not data:
            raise ConnectionError("the model closed the connection")
        self.buf += data

    def chat(self, messages):
        body = json.dumps({"model": "fake", "messages": messages, "tools": TOOLS}).encode()
        head = (
            "POST /v1/chat/completions HTTP/1.1\r\nHost: model\r\n"
            "Content-Type: application/json\r\nContent-Length: %d\r\n\r\n" % len(body)
        )
        t0 = time.perf_counter_ns()
        self.sock.sendall(head.encode() + body)
        while b"\r\n\r\n" not in self.buf:
            self.recv()
        head, _, self.buf = self.buf.partition(b"\r\n\r\n")
        lines = head.decode().split("\r\n")
        if not lines[0].startswith("HTTP/1.1 200"):
            raise RuntimeError(lines[0])
        length = 0
        for line in lines[1:]:
            name, _, value = line.partition(":")
            if name.lower() == "content-length":
                length = int(value)
        while len(self.buf) < length:
            self.recv()
        answer, self.buf = self.buf[:length], self.buf[length:]
        self.waited.append(time.perf_counter_ns() - t0)
        return json.loads(answer)


class Offline:
    """A canned model, for packing the standard library."""

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


def turn(model, messages, label, variant=0):
    """One turn: send the conversation, run the tool it asks for, append."""
    answer = model.chat(messages)
    message = answer["choices"][0]["message"]
    call = message["tool_calls"][0]
    args = json.loads(call["function"]["arguments"])
    result = compute(args["n"], args["seed"] + variant)
    messages.append({"role": "assistant", "content": None, "tool_calls": [call]})
    messages.append({"role": "tool", "tool_call_id": call["id"], "content": json.dumps(result)})
    return "%s %s n=%d seed=%d value=%d notes=%d sha=%s conn_requests=%d" % (
        label,
        call["id"],
        args["n"],
        args["seed"],
        result["value"],
        result["notes"],
        result["sha"],
        answer["x_requests_on_connection"],
    )


def branch(model, messages, b):
    """A branch: the last tool result explored with variant `b`, with notes
    and a connection of its own, then one more turn."""
    global NOTES
    with open(NOTES, "rb") as f:
        notes = f.read()
    NOTES = "agent-notes-%d.txt" % b
    with open(NOTES, "wb") as f:
        f.write(notes)
    call = messages[-2]["tool_calls"][0]
    args = json.loads(call["function"]["arguments"])
    result = compute(args["n"], args["seed"] + b)
    messages[-1] = {"role": "tool", "tool_call_id": call["id"], "content": json.dumps(result)}
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
        {"role": "user", "content": "Explore the sums and keep notes."},
    ]
    spent = []
    for t in range(1, turns + 1):
        t0 = time.perf_counter_ns()
        line = turn(model, messages, "turn %d" % t)
        spent.append(time.perf_counter_ns() - t0)
        print(line, flush=True)
        time.sleep(PAUSE)  # between turns: the host may suspend the cell here
    if timed:
        print("agent turn_ns=%s" % ",".join(map(str, spent)), flush=True)
        print("agent model_ns=%s" % ",".join(map(str, model.waited)), flush=True)
    print("fork", flush=True)
    if mode == "linux":
        fork_linux(model, messages, timed)
        return
    time.sleep(3600)  # the host freezes the cell here and starts the branches
    with open("/branch", "rb") as f:
        b = int(f.read())
    print("branch %d" % b, flush=True)
    sys.stdout.write(branch(model, messages, b))


def main():
    if len(sys.argv) < 5:
        "model".encode("idna")  # what `getaddrinfo` does with a host name
        print(turn(Offline(), [], "offline"))
        return
    host, port, mode, turns = sys.argv[1:5]
    run(Model(host, int(port)), mode, int(turns), sys.argv[5:] == ["timed"])


if __name__ == "__main__":
    main()
