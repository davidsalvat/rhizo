# Proposal

## Why

A real `rhizo chat` session against a local 7B model (Ollama,
`qwen2.5:7b`) on a VPS showed three faults of the conversational agent and
of the chat:

1. **A snippet ending in an expression returns nothing.** The model sent
   `import math`, `result = 18 * 8`, `sqrt_result = math.sqrt(result)`,
   `sqrt_result`. `run_python` compiles in `"exec"` mode, which drops the
   value of a final bare expression, so the model read `(no output)` and
   made up a wrong answer (10.95; the answer is 12). Models write snippets
   as they would type them into the Python REPL.
2. **Tool calls written as text are shown as the answer.** The 7B model
   sometimes writes its call into the message content
   (`Ronaldo {"name": "run_python", "arguments": {"code": "..."}}`) and
   sends no `tool_calls`. The agent printed that JSON as the answer and the
   tool never ran.
3. **A line typed during a turn is queued silently.** `rhizo chat` reads
   the next line only after the turn ends, so a line typed meanwhile waits
   in the terminal, is sent as the next message without a word, and the
   prompt is printed again in front of a line already typed
   (`[2]> [2]>`).

## What Changes

- **`run_python` behaves like the REPL**: when a snippet's last statement
  is an expression, it is evaluated after the rest, and its value's `repr`
  is appended to the output unless it is None.
- **A tool call written as text runs as a tool call**: when a reply has no
  `tool_calls` and its content holds a JSON object that parses, names a
  known tool and has an object of arguments, the agent runs the first such
  object as a call (as several agent frameworks do for Qwen) and does not
  print the content as an answer. Nothing else is taken for a call.
- **`rhizo chat` says when a line waits**: lines are read on a thread of
  their own; one typed while the chat works on a line prints
  `[rhizo] queued for the next turn` at once and is sent when the turn
  ends, echoed after the prompt instead of a second, empty prompt.
- **Tests**: the agent's self-check (run whenever the CPython cell's files
  are packed) covers both agent fixes; the fake model answers a "square
  root" message with a snippet ending in an expression and an "as text"
  message with a call written into its content; the `control` step types
  those two messages at once and checks the tool result `12.0`, the
  queued note and the transcript; a new differential check runs the chat
  agent on Linux over the same messages; a host test of the CLI's queue.

### Out of scope

Calls written as text in other formats (`<tool_call>` tags, several calls
in one reply); interrupting a running turn.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workloads`: the snippet tool shows a last expression's value; a tool
  call written as text runs.
- `control-plane`: `rhizo chat` says when a typed line is queued.

## Impact

`cells/python/agent_chat.py`, `cli/src/chat.rs`, `xtask/src/hostnet.rs`
(fake model), `xtask/src/control.rs` (`control` step),
`xtask/src/differential.rs`, `cells/expected.rs`. The snippet tool now
parses each snippet to an AST before compiling it (about 20 µs more per
snippet natively), inside the turns `chat.turn_us` measures.
