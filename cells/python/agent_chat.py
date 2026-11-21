# The conversation mode of the CPython agent (spec `workloads`, change
# `add-interactive-chat`): it reads what the user types from its standard
# input, one message per line, and answers each through an OpenAI-compatible
# chat-completions API (`Model` of `agent_loop.py`: HTTP/1.1, one kept-alive
# connection), calling tools that run here, inside the cell, as often as the
# model asks. The whole conversation lives in this process, so forking or
# suspending the cell carries it.
#
#     python3 agent_chat.py URL MODEL
#
# What it prints, for `rhizo chat` (cli/src/chat.rs) to show:
# - the model's answers, whole; each tool call as `  [tool] NAME`, its
#   arguments and its result indented below;
# - `\x1eready` once it reads its input, and after each answer
#   `\x1eturn model_ns=M agent_ns=A`: the time spent waiting for the model
#   (each request sent until its answer is read) and the whole turn here,
#   from the line read to that line. `\x1e` never starts another line.
#
# It ends at the end of its input. A connection the model closed, or one a
# fork or a resume reset, is opened again once per request. Without
# arguments it runs its tools once offline (how its standard library is
# packed, cells/python/pack_stdlib.py: what they import is packed with it).
import contextlib
import io
import json
import os
import sys
import time
import traceback

# Modules a snippet may import (packed because this script imports them).
import collections  # noqa: F401
import datetime  # noqa: F401
import random  # noqa: F401
import re  # noqa: F401
import textwrap  # noqa: F401

sys.dont_write_bytecode = True  # no `__pycache__` next to `agent_loop.py`
from agent_loop import Model  # noqa: E402

# Most model requests per message (each tool round is one).
STEPS = 8
# A snippet's limits: wall time, and characters of output kept.
SNIPPET_SECONDS = 10.0
OUTPUT_CAP = 4000
# Most characters of a file `read_file` returns.
READ_CAP = 16000
SYSTEM = (
    "You are a helpful agent running inside an isolated Rhizo cell. "
    "Your tools run inside the cell: Python snippets, and files in its own "
    "file system. Use them when they help, then answer briefly."
)


def tool(name, description, **params):
    return {
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": {
                "type": "object",
                "properties": {k: {"type": "string", "description": v} for k, v in params.items()},
                "required": list(params),
            },
        },
    }


TOOLS = [
    tool(
        "run_python",
        "Run a short Python 3 snippet in the cell and return what it prints "
        "(%d characters at most, %d seconds at most)." % (OUTPUT_CAP, SNIPPET_SECONDS),
        code="The Python code; print() what you need back.",
    ),
    tool("read_file", "Read a text file of the cell.", path="Its path."),
    tool("write_file", "Create or replace a text file of the cell.", path="Its path.", content="Its text."),
    tool("list_dir", "List a directory of the cell (`/` for the root).", path="Its path."),
]


class Capped(io.TextIOBase):
    """A text stream that keeps the first `OUTPUT_CAP` characters."""

    def __init__(self):
        self.kept, self.dropped = [], 0
        self.size = 0

    def writable(self):
        return True

    def write(self, text):
        room = OUTPUT_CAP - self.size
        self.kept.append(text[:room])
        self.size += min(len(text), max(room, 0))
        self.dropped += max(len(text) - max(room, 0), 0)
        return len(text)

    def text(self):
        out = "".join(self.kept)
        if self.dropped:
            out += "\n[%d more characters dropped]" % self.dropped
        return out


class TimeUp(Exception):
    pass


def run_python(code):
    """Runs `code` in a namespace of its own, its output captured and
    capped, stopped at the first line it runs after `SNIPPET_SECONDS` (a
    line-by-line trace: one long call into C is not interrupted)."""
    out = Capped()
    deadline = time.monotonic() + SNIPPET_SECONDS

    def trace(frame, event, arg):
        if time.monotonic() > deadline:
            raise TimeUp()
        return trace

    try:
        compiled = compile(code, "<snippet>", "exec")
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
            sys.settrace(trace)
            try:
                exec(compiled, {"__name__": "__snippet__"})
            finally:
                sys.settrace(None)
    except TimeUp:
        out.write("%s[stopped after %d seconds]" % ("\n" if out.size else "", SNIPPET_SECONDS))
    except BaseException:
        out.write(traceback.format_exc(limit=-3))
    return out.text() or "(no output)"


def read_file(path):
    with open(path, encoding="utf-8", errors="replace") as f:
        text = f.read(READ_CAP + 1)
    return text[:READ_CAP] + ("\n[truncated]" if len(text) > READ_CAP else "")


def write_file(path, content):
    folder = os.path.dirname(path)
    if folder:
        os.makedirs(folder, exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(content)
    return "wrote %d characters to %s" % (len(content), path)


def list_dir(path):
    names = []
    for name in sorted(os.listdir(path or "/")):
        names.append(name + "/" if os.path.isdir(os.path.join(path or "/", name)) else name)
    return "\n".join(names) or "(empty)"


RUN = {"run_python": run_python, "read_file": read_file, "write_file": write_file, "list_dir": list_dir}


def call_tool(call):
    """Runs one tool call; returns its name, its arguments and its result
    (an error is a result too: the model reads it)."""
    function = call.get("function") or {}
    name = function.get("name", "")
    raw = function.get("arguments") or "{}"
    try:
        args = raw if isinstance(raw, dict) else json.loads(raw)
        if not isinstance(args, dict):
            raise ValueError("not an object")
    except ValueError:
        return name, {}, "error: the arguments are not a JSON object"
    run = RUN.get(name)
    if run is None:
        return name, args, "error: no tool named %r" % name
    try:
        return name, args, run(**{k: str(v) for k, v in args.items()})
    except TypeError as e:
        return name, args, "error: %s" % e
    except OSError as e:
        return name, args, "error: %s" % e


def indented(text, prefix="      "):
    return "\n".join(prefix + line for line in str(text).splitlines() or [""])


def ask(model, messages):
    """One request, on a new connection if the last one is gone."""
    for attempt in (0, 1):
        try:
            if model.sock is None:
                model.connect()
            return model.chat(messages, TOOLS)
        except OSError:
            if model.sock is not None:
                model.sock.close()
            model.sock = None
            if attempt:
                raise


def answer(model, messages):
    """Model requests and tool calls until the model answers in text."""
    for _ in range(STEPS):
        message = ask(model, messages)["choices"][0]["message"]
        calls = message.get("tool_calls") or []
        if not calls:
            text = message.get("content") or ""
            messages.append({"role": "assistant", "content": text})
            print(text.replace("\x1e", "").strip() or "(no answer)")
            return
        messages.append({"role": "assistant", "content": message.get("content"), "tool_calls": calls})
        for call in calls:
            name, args, result = call_tool(call)
            print("  [tool] %s" % name)
            for key, value in args.items():
                print(indented(value) if len(args) == 1 else "    %s:\n%s" % (key, indented(value)))
            print("  [tool] ->")
            print(indented(result))
            messages.append({"role": "tool", "tool_call_id": call.get("id", "call"), "content": result})
    print("[agent] stopped after %d model requests" % STEPS)


def chat(model):
    messages = [{"role": "system", "content": SYSTEM}]
    print("\x1eready")
    while line := sys.stdin.buffer.readline():
        t0 = time.perf_counter_ns()
        waited = len(model.waited)
        text = line.decode("utf-8", errors="replace").strip()
        if text:
            messages.append({"role": "user", "content": text})
            try:
                answer(model, messages)
            except Exception as e:  # noqa: BLE001 (the model or its server failed)
                print("[agent] the model failed: %s" % " ".join(str(e).split())[:300])
                while messages[-1]["role"] != "user":
                    messages.pop()
                messages.pop()
        model_ns = sum(model.waited[waited:])
        print("\x1eturn model_ns=%d agent_ns=%d" % (model_ns, time.perf_counter_ns() - t0))


def main():
    if len(sys.argv) < 3:
        for call in (
            {"function": {"name": "run_python", "arguments": '{"code": "print(6 * 7)"}'}},
            {"function": {"name": "write_file", "arguments": '{"path": "chat-check.txt", "content": "x"}'}},
            {"function": {"name": "read_file", "arguments": '{"path": "chat-check.txt"}'}},
            {"function": {"name": "list_dir", "arguments": '{"path": "."}'}},
        ):
            call_tool(call)
        os.remove("chat-check.txt")
        return
    sys.stdout.reconfigure(line_buffering=True)
    chat(Model(sys.argv[1], sys.argv[2]))


if __name__ == "__main__":
    main()
