# Proposal

## Why

MicroPython (H10) showed that an interpreter runs in a cell, but agents run
CPython: its start-up (path computation, frozen and compiled modules,
encodings), its standard library (`json`, `re`, `datetime`, `random` and
their C accelerators) and its memory behavior are what an agent sandbox
must start fast and fork cheaply. Milestone H16 runs real, unmodified
CPython in a cell and measures an agent-like request cold and from a warm
template, next to CPython on Linux in the same VM.

## What Changes

- **CPython built as a static cell**: `xtask` builds the pinned release
  CPython 3.13.18 (PSF License 2.0, permissive) as a static executable when
  its source tarball is given (`RHIZO_CPYTHON_SRC`) and checks the tarball's
  SHA-256. The C modules an agent request uses are built in (`_json`,
  `_struct`, `math`, `_random`, `_datetime`, `binascii`, `zlib`, `_bisect`,
  `_heapq`; `_sre` and the rest of the core are always in). CI downloads the
  tarball in the jobs that build cells and caches the build; without it,
  every CPython check reports that it did not run, as for MicroPython.
- **The standard library in the cell's files**: the modules the workload
  and the interpreter's start-up import, compiled to sourceless `.pyc`
  files under `/lib/python3.13`, where CPython finds them from its
  executable `/bin/python3` with no environment. `xtask` encodes them, with
  the script under `/app`, as a cell file system (the encoding snapshots
  use); the kernel decodes it once and every CPython cell gets a
  copy-on-write clone.
- **Workload**: `cells/python/agent_demo.py`, an agent-like tool call:
  parse a JSON request, run regular expressions and string operations over
  its text, compute (dates, statistics, a CRC), answer in JSON. Its output
  on Linux is the expectation (`cells/expected.rs`), checked by the
  differential check with the same CPython and files.
- **Linux ABI**: whatever CPython needs that cells lack, each call with host
  tests and Linux semantics (found by running it; calls it uses and Rhizo
  does not implement are reported as before).
- **In-kernel test**: CPython answers in a cell as on Linux; cells forked
  from a template frozen after the warm-up answer without warming up.
- **Benchmarks**: `workload.cpython_cold_us` (exec to answer: interpreter
  start, imports, warm-up, answer) and `workload.cpython_warm_us` (a fork of
  the template, to the answer), and the same two measured with CPython as a
  Linux process in the same Firecracker VM (cold: fork and exec; warm: fork
  of a CPython process that imported and warmed up the script).

### Out of scope

Threads in CPython's own use (`threading`, `concurrent.futures`),
`subprocess`, sockets, `ctypes` and loading C extensions (`dlopen`),
`site-packages`, writing `__pycache__` files, and the README's images (they
are regenerated when the change is archived).

## Capabilities

### Modified Capabilities

- `workloads`: CPython in a cell.
- `benchmarks`: CPython workload metrics.
- `comparisons`: CPython as a Linux process in the same VM.
- `linux-abi`: calls CPython needs.

## Impact

`xtask/src/cpython.rs` (new), `xtask/src/{main,differential,bench,compare,
refguest,deps}.rs`, `xtask/Cargo.toml` (depends on `rhizo-fs`),
`kernel/build.rs`, `kernel/src/{cell,tests}.rs`, `crates/rhizo-abi`,
`cells/python/`, `cells/expected.rs`, `bench/host/`, `bench/README.md`,
`README.md`, `AGENTS.md`, CI.
