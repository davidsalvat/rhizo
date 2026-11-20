# Proposal

## Why

Agents run Python. The proof of concept runs static C programs, which shows
the Linux ABI works but not that an interpreter does: start-up, imports from
files, a heap that grows, a real event loop's sleeps. MicroPython (MIT) is a
real Python interpreter that builds as a static executable.

## What Changes

- `xtask` builds MicroPython v1.24.1 (pinned commit) as a static cell when
  its source is given (`RHIZO_MICROPYTHON_SRC`); CI fetches it. Threads,
  sockets, FFI, TLS and readline are left out.
- `cells/python/rhizo_demo.py`: an agent-style request (parse JSON, compute,
  answer) whose output on Linux is the expectation; it pauses after a
  warm-up so a host can freeze it as a template.
- In-kernel test: MicroPython imports the module from the cell's files and
  prints what it prints on Linux; cells forked from a template frozen after
  the warm-up answer without warming up. The differential check runs it
  natively.
- `select` and `pselect6` with no descriptors sleep (MicroPython's
  `time.sleep`); with descriptors they return `ENOSYS`.
- Benchmarks: `workload.python_cold_us` and `workload.python_warm_us`.

### Out of scope

CPython, threads, sockets, `select` on descriptors.

## Capabilities

### Modified Capabilities

- `workloads`: a real interpreter.
- `linux-abi`: `select`/`pselect6` as sleeps.
- `benchmarks`: Python workload metrics.

## Impact

`xtask/src/main.rs`, `xtask/src/differential.rs`, `kernel/build.rs`,
`kernel/src/cell.rs`, `kernel/src/tests.rs`, `crates/rhizo-abi`,
`cells/python/`, `cells/expected.rs`, CI.
