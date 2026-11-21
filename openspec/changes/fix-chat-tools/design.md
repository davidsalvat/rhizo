# Design

## D1. The REPL's last expression, with `ast`

`snippet()` parses the code with `ast.parse`; when the last statement is an
`ast.Expr`, it is popped and compiled alone in `"eval"` mode, the rest in
`"exec"` mode. Both run in the same namespace, under the same trace (so the
time limit also stops a slow expression) and the same captured output; a
value that is not None is printed with `repr`, as the REPL does. Only the
last statement counts: an expression inside a loop or a function shows
nothing, as in a script. A snippet whose last line is `print(...)` is
unchanged (its value is None). `ast` is imported by the script, so the
CPython cell packs it (`pack_stdlib.py`).

Cost: parsing to an AST and compiling it takes about 33 µs for the fake
model's one-line snippet against 15 µs for `compile` alone (host,
CPython 3.13.18), once per tool call.

## D2. Tool calls written as text

`text_call(text)` scans the content for `{` and tries
`json.JSONDecoder.raw_decode` at each; the first object that parses, whose
`name` is one of the agent's tools and whose `arguments` is an object, is
the call. Only when the reply has no `tool_calls`. It becomes a normal
call (`id` `call_text`) in the conversation: the assistant message carries
it as `tool_calls` with no content, so the tool's result answers it and
the next request is well formed for any OpenAI-compatible server. The text
around the JSON is dropped, not printed: the user sees the call and its
result as for any call, then the model's real answer. Strict on purpose:
an unknown name, arguments that are not an object, or JSON that does not
parse leave the reply a plain answer.

## D3. Queued lines: in `rhizo chat`, not in the agent

The line typed during a turn never reaches the cell while the turn runs:
`rhizo chat` sends a line, then blocks reading the cell's output until the
turn's timing line, and only then reads its own input again. The typed
line waits in the terminal. The agent cannot see it, so the note belongs
to the CLI, the only side that holds the line.

`rhizo chat` now reads its input on a thread that sends each line over a
channel. A small `Queue` (shared under a mutex) counts lines read but not
yet taken and whether the chat is working on one; a line read while busy
or behind another waiting line prints `[rhizo] queued for the next turn`
at once. When the chat is done with a line and another is already
waiting, no prompt is printed; on a terminal the waiting line is echoed
after the prompt (`[N]> line`), so the prompt appears once per line. Input
that is not a terminal is echoed (`> line`) as before.
