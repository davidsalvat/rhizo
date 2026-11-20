# Proposal

## Why

The README compares a cell, born inside a kernel that is already running,
with a Firecracker microVM and a gVisor sandbox, each of which starts a
whole VM or sandbox process. A cell is closer to a process than to a VM,
so most of the "≈ 4,200× faster" figures compare different things. The
lazy resume figure (0.87 µs) also hides that pages come back later, on
first touch.

The fair comparison for the kernel mechanisms is Linux doing the same
operation on processes, inside the same kind of VM. The fair comparison
for suspend and resume of a process is CRIU. The README must lead with
those, and show the VM and sandbox figures as a separate tier.

## What Changes

- `cargo xtask bench` measures Linux processes **inside the Firecracker
  guest**, the VM Rhizo's own figures come from (same VMM, same
  virtualization), under guest `linux-process`, vmm `firecracker`:
  - `process.fork_call_us`: `fork()` returning in a parent that holds
    50 MiB written (pairs with `cell.fork_us`);
  - `process.fork_touch_us`: that fork plus the child writing every page,
    exiting, and the parent waiting (pairs with `cell.fork_touch_us`);
  - `process.spawn_us`: `posix_spawn` of the same static `hello` until it
    exits (pairs with `cell.run_us`).
- The host process baselines gain `process.fork_call_us` and
  `process.fork_touch_us` too.
- `cargo xtask bench` measures CRIU, where it is installed and runs:
  `snapshot.restore_ms` for guest `criu`, vmm `host`: restoring a dumped
  process holding the same memory as the cell snapshot, until it runs.
- The README leads with the fair pairs ("the same operation, same kind of
  VM"), shows resume three ways (lazy, eager, lazy plus touching every
  page), and keeps VM and sandbox figures in their own tier.

### Out of scope

CRIU inside the guest; new cell mechanisms.

## Capabilities

### Modified Capabilities

- `comparisons`: Linux processes inside the same VM; CRIU.

## Impact

`bench/host/linux_process.c`, `xtask/src/compare.rs`, `xtask/src/refguest.rs`,
`xtask/src/bench.rs`, `docs/img/gen.py`, `.github/workflows/ci.yml`,
`bench/README.md`, README.
