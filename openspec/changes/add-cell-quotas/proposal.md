# Proposal

## Why

One cell may take all memory down to the kernel's reserve, any amount of
CPU time, 1024 descriptors and as many sockets as the kernel can hold
(open item O1 of `docs/security.md`). The kernel survives all of it, but
a single misbehaving agent then starves every other cell on the node: the
host has no way to say how much one cell may use.

## What Changes

- `rhizo_cell::quota::Quota`: memory (bytes of pages and page tables),
  CPU time (nanoseconds over the cell's life), descriptors and sockets.
  `Quota::NONE` is today's behavior. It lives in the `Cell`, with the CPU
  time used so far, so forks, templates and snapshots carry it.
- `Host::set_quota(id, quota)` sets a live cell's quotas.
- Memory: every frame taken for the cell (pages, page tables, private
  copies of shared pages, lazily restored pages, stack growth) is counted
  through `quota::Metered`; past the quota the allocation fails as when
  memory runs out: `mmap` and `brk` answer `ENOMEM`, a fault that needs a
  frame kills the cell, as today when memory runs out.
- CPU time: counted per slice, system calls included; a cell past its
  quota is ended (`Ending::OutOfCpu`, reported in its output) at the end
  of the slice that used it up.
- Descriptors: past the quota, `EMFILE` (as past 1024 today; `dup2` to a
  descriptor past it answers `EBADF`, as Linux past `RLIMIT_NOFILE`).
- Sockets: past the quota, socket creation answers `ENFILE`.
- `getrlimit`/`prlimit64` report the quotas (`RLIMIT_CPU`, `RLIMIT_AS`,
  `RLIMIT_NOFILE`).
- Snapshot format version 7: the state section holds the quotas and the
  CPU time used. A resumed cell keeps both; a fork or template copy keeps
  the quotas and starts with no usage.

### Out of scope

Cells setting their own limits (`setrlimit` stays a no-op), file data
(already bounded per cell: 64 MiB of data and 4096 names), kernel heap
used for a cell's threads and sockets (bounded by the counts), and
hierarchies of quotas shared by several cells.

## Capabilities

### Modified Capabilities

- `isolation`: host-set quotas per cell.
- `snapshots`: a snapshot keeps the cell's quotas and the CPU time used.

## Impact

`crates/rhizo-cell` (`quota`, `Cell`), `crates/rhizo-fs` (limits on
descriptors and sockets), `crates/rhizo-abi` (`getrlimit`),
`crates/rhizo-snapshot` (version 7), `kernel/src/cell.rs`,
`kernel/src/tests.rs`, `cells/src/bin/limits.rs`, `docs/security.md`.
The cost on benchmarked paths is a compare per frame taken and one clock
read per slice.
