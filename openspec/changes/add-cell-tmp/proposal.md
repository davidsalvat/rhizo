# A /tmp in the CPython cell

## Why
Trying `rhizo chat` with a real model, the agent's `run_python` tool wrote
`/tmp/suma.txt` and failed with `FileNotFoundError`: the CPython cell's
files have `/usr/lib/python3.13` and `/app` but no `/tmp`, and programs (and
the models that write them) expect one.

## What Changes
- The CPython cell's packed files gain an empty `/tmp` directory.
- docs/demo.md says which directories the agent's cell has, that Ctrl-D
  ends a branch for good (`/suspend` keeps it), and that small models often
  answer without calling tools.

## Impact
- Specs: `workloads` (the CPython cell's files).
- Code: `xtask/src/cpython.rs` (and its host test); docs/demo.md.
