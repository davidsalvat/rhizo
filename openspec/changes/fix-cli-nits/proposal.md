# Proposal

## Why

A run of `rhizo` on a KVM host (change `add-control-plane`) found three
nits:

1. `rhizo up` compiled the kernel again every time, even with nothing
   changed. The kernel's build script told cargo to watch every embedded
   cell file, the optional ones too (`micropython`, `cpython`,
   `cpython-files`, `distro-files`); `xtask` removes an optional cell that
   is not built, and cargo treats a watched file that is missing as
   changed, so the script ran, and the kernel was compiled, on every build.
2. Fork identifiers skipped: after a suspend and resume of cell `N`,
   `rhizo fork N` gave `N+2`. The control kernel's table of cells gave the
   host the kernel's own cell identifiers, and a resume takes one (the
   resumed cell is a new kernel cell, which the table hides).
3. `~/.rhizo` kept the booted kernel image (5 MiB and more) and the
   server's log after `rhizo down`, and a server that did not end cleanly
   left its state file and socket for good.

## What Changes

- The kernel's build script watches an optional cell only when it is
  built, and the variables that make `xtask` build them
  (`RHIZO_MICROPYTHON_SRC`, `RHIZO_CPYTHON_SRC`). The `control` test step
  fails if building the control kernel again, unchanged, compiles it.
- The control kernel numbers the cells the host starts or forks itself,
  from 1, one after the other; a resume takes no identifier. The
  `control` test step checks that a fork after a resume takes the next
  identifiers.
- After `rhizo down` the state directory holds only `console.log`, the
  last VM's console; `rhizo up` first removes what an earlier server left.
  The `control` test step checks the directory after `down`.

### Out of scope

The Firecracker configuration file `xtask` writes next to its tools (one
per server process, outside the state directory).

## Capabilities

### Modified Capabilities

- `control-plane`: identifiers of cells, the build `rhizo up` reuses, and
  what the state directory keeps.

## Impact

- `kernel/build.rs`, `kernel/src/control.rs`,
  `cli/src/{main,serve,state,args}.rs`, `xtask/src/{lib,control}.rs`,
  `docs/demo.md`.
- An unchanged `rhizo up` (and any unchanged `cargo xtask` kernel build)
  skips about 10 to 20 s of compiling; no benchmarked path moves.
