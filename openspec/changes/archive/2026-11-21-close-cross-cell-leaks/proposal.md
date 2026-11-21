# Proposal

## Why

The design of `add-cell-hierarchy` (design D12) lists 40 kernel resources
that more than one cell can touch, as possible channels between cells
that hold no handle to each other. The hierarchy closes most of them in
phases. Three of them leak today, cost little to close, and need none of
the hierarchy:

- **`sysinfo` reports the kernel's free memory** (item 6). One cell
  allocates and frees; another reads `sysinfo`: a storage channel of many
  bits per second, and a view of every other cell's memory use.
- **Ephemeral ports come from one counter for all cells** (item 2). The
  distance between two ports a cell takes counts the ports every other
  cell took in between: a storage channel, and a view of other cells'
  connection activity.
- **Any cell can empty the whole page cache** (item 9). The cache of file
  pages holds at most 65,536 pages, and the page past the limit empties
  it; memory pressure empties it too, and a cell without a memory quota
  creates memory pressure by allocating. Map latency then shows whether a
  page was cached: the eviction half of a probe-and-flush timing channel,
  at the cost of one allocation loop. (Emptying the cache also gave the
  frames of mapped pages to their mappers without counting them against
  their quota.)

## What Changes

- **`sysinfo` shows the cell only itself** (design D1): total memory is
  the cell's memory quota (at most the memory cells share), free memory is
  what its own use leaves of it, processes are its own threads. No load,
  no swap, as before.
- **Ephemeral ports per cell** (design D2): each cell takes ports in order
  from a start of its own, which a keyed hash of the cell's identifier
  under the kernel's secret chooses (with the boot, as before). What other
  cells bind no longer shows in a cell's ports. A resumed or forked cell
  has a new identifier, hence a new start: no cell reuses the ports it
  closed just before, and a new boot still starts elsewhere
  (`fix-ephemeral-ports`). The kernel's own sockets keep their counter.
- **No cell evicts pages another cell maps** (design D3): a full cache no
  longer empties itself; it drops only the pages no file holds any more
  (nobody can map them again), and a page it has no room for is the
  caller's own copy, counted in its quota. Memory pressure drops only the
  kept pages whose dropping frees memory: those no mapping uses, and those
  no file holds.
- Findings S24 (sysinfo), S25 (ports) and S26 (page cache) in
  `docs/security.md`, fixed, with hostile tests: in the kernel, with two
  cells each, and on the host.

### Out of scope

The hierarchy itself, per-tree loopback networks, egress policy, handles
(the phases of `add-cell-hierarchy`). Partitioning the port space and the
page cache per domain (its items 2 and 9): ports in use are still one
namespace, so a cell still learns that a port it tried is held, and a page
another cell maps is still found in the cache (the deduplication half of
item 9). Cell identifiers (item 19): `getpid` still counts the cells the
host created, but only the host creates cells today, so no cell can write
that channel; numbering per domain belongs with domains. Global `ENOMEM`,
`ENOSPC` and `ENFILE` (items 5, 7, 8, 24): budgets per domain, phase 1.

## Capabilities

### Modified Capabilities

- `linux-abi`: `sysinfo` reports the cell's own memory and threads.
- `networking`: ephemeral ports per cell.
- `cells`: the page cache drops only pages no other cell can be using.
- `isolation`: no cell observes another through `sysinfo`, ports or the
  page cache.

## Impact

`crates/rhizo-abi` (`sysinfo`, `Services::memory`, `Abi`'s ports,
`PageCache`), `crates/rhizo-net` (`Ports`, `Net::enter`, `Net::leave`),
`crates/rhizo-mm` (`Frames::metered`), `crates/rhizo-cell` (`Metered`,
`CountingFrames`), `crates/rhizo-fs` (`Page::is_held_elsewhere`),
`kernel/src/cell.rs` (`KernelServices::memory`, `KernelPages`,
`drop_images`), `kernel/src/tests.rs`, `cells/src/bin/observer.rs`,
`docs/security.md`, `docs/audit-scope.md`. Benchmarked paths: only the
calls that can bind a port enter and take back the cell's cursor; a file
page that misses the cache when it is full looks through it at most once
every 4,096 misses.
