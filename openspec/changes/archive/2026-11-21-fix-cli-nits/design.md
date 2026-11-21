# Design

## D1. Watch only the cells that are built

A missing file that a build script names in `rerun-if-changed` is always
stale for cargo, so the script runs, and the crate is compiled, on every
build. Options:

| Option | Rebuilds when a cell appears | Cost |
|---|---|---|
| **Watch built cells and the variables that build the optional ones (chosen)** | yes: `xtask` builds MicroPython and CPython only when `RHIZO_MICROPYTHON_SRC`, `RHIZO_CPYTHON_SRC` are set, and `distro-files` depends on the host and target alone | none |
| `xtask` writes an empty file instead of removing an optional cell | yes | every `exists()` check on cells in `xtask` changes meaning |
| Watch the cells' directory | yes | cargo scans the whole directory, Rust cells' build files included |

## D2. The table numbers its cells

The control kernel's table already hid the kernel cell behind each entry
(a resumed snapshot runs as a new kernel cell). It now also chooses the
identifier the host sees: a counter from 1, taken by `run` and by each
child of a `fork`, never by a resume. Kernel cell identifiers stay internal
(sockets, matching ended cells). The protocol does not change.

## D3. What the state directory keeps

`Dir::clean` removes the files a VM leaves (`state.new`, the socket, the
kernel copy, `server.log`, then the state file, which `rhizo down` waits
for) and keeps `console.log`, the one file worth reading after the VM is
gone. The server calls it when the VMM ends; `rhizo up` calls it before
starting, for a server that was killed. A server that dies mid-run leaves
its log for diagnosis until the next `up`.
