# Design

The threat is the one of `add-cell-hierarchy` design D0: two cells that
must not communicate, holding no handle to each other, each running
hostile code. Here, a resource leaks when one cell can change it and
another can read it (a storage channel) or time it (a timing channel).
This change removes three that cost little, and leaves the rest to the
hierarchy's phases.

## D1. `sysinfo` reports the cell, not the machine

Before: `totalram` was the memory cells share, `freeram` the frames free
in the kernel's allocator, `procs` 1. A cell reading `freeram` sees every
other cell's allocations.

Now `Services::memory` returns the calling cell's limit and use:

- `totalram` = min(memory cells share, the cell's memory quota);
- `freeram` = `totalram` minus the frames counted against the cell's
  quota, 0 when it holds more (inherited from a fork);
- `procs` = the cell's threads (Linux counts tasks; a cell is one
  process);
- loads, swap and buffers stay 0, uptime stays boot-relative (no cell can
  change it: item 35 of the inventory).

The quota's meter (`add-cell-quotas`) already counts the cell's frames.
`KernelServices` gets the cell's meter for system calls; the total reads
the allocator's usable frames, fixed at boot. A cell without a quota sees
the whole of memory as its total and only its own use taken from it, as a
container sees its cgroup: `freeram` can then promise more than is free
when other cells hold memory. That is the price of not telling it; the
hierarchy's reserved budgets (D4 there) make the number true.

Nothing else reports global state: there is no `/proc`; `getrusage` and
`times` are not implemented (`ENOSYS`); `getrlimit`/`prlimit64` report the
cell's own quota; `uname` and `sched_getaffinity` are constant.

Lost: programs that size caches from `freeram` see more than is free when
cells without a quota share the machine.

## D2. Each cell walks the ephemeral ports from its own start

Before: one `next_port` for all; `bind(0)` and implicit binds took the
next free port in order. Two binds by one cell show how many ports all
others took between them.

Considered:

- **A random port per bind** (RFC 6056 algorithm 1), from a keyed hash of
  a global counter: hides the counter without any per-cell state. Two
  boots then reuse a port of each other's first connections with
  probability about m²/28,232 (2 % for 25 connections), against 2m/28,232
  (0.2 %) today: that brings back the 1 s stall `fix-ephemeral-ports`
  removed, at a rate the gated agent turns would see. Rejected.
- **A counter per socket owner kept in `Net`**: needs an owner on every
  socket and a hook to forget the counter when a cell ends, or its table
  grows without end. Rejected for the next option, which needs neither.
- **The cell's own cursor, carried with its calls**: chosen.

`rhizo_net::Ports` is a cell's cursor: its key (the cell's identifier) and
the next port to try (0 before the first). The kernel keeps it in the
cell's `Live` record. `CallNet`, which locks the sockets the first time a
call uses them, calls `Net::enter(ports)` under the lock; at the end of
the call the kernel takes them back with `Net::leave()`. `Net::ephemeral`
uses the entered cursor: its first port is

    32768 + (siphash24(secret, key) + mix(boot)) mod 28232

and later ports go on in order, skipping ports in use, as before. Calls
with no cursor entered (the kernel's own transfer sockets, host tests)
use `next_port` as before, so the order within one cell, and every
existing host test, are unchanged.

Properties:

- What other cells bind does not move a cell's ports. They only show
  when another cell *holds* the port a cell would take next (it is
  skipped): ports in use remain one namespace until the hierarchy gives
  each tree its own loopback network (item 1 and 2, phase 1).
- A cell cannot compute another cell's start: SipHash under the kernel's
  secret (the one parked keys use, finding S15).
- A resumed or forked cell gets a new identifier, hence a new start: it
  does not reuse the ports it closed just before, which a peer may keep
  in `TIME_WAIT`. Two consecutive boots collide only as often as before
  per cell (each cell's ports are a run from a random start).
- The cursor is not in a snapshot: a resumed cell is a new cell to the
  sockets, as after a new boot (rule 6: the cursor is plain data, 8
  bytes, and recomputed rather than restored).

Cost: two copies of 8 bytes per call that uses sockets; nothing on calls
that do not.

## D3. No cell evicts a page another cell maps

The page cache of `add-dynamic-linking` (D1 there) keeps file pages by
content identity (block pointer, length), shared by every cell that maps
the same bytes. Two ways emptied all of it:

1. The page past its 65,536 pages emptied it. A cell writes 256 MiB of
   its own files and maps them.
2. Memory pressure: when a frame cannot be had, `drop_images` empties
   the image cache and the page cache. A cell without a memory quota gets
   there by allocating.

Either is the "flush" of a flush-and-probe channel: the victim's next map
of a library page misses (a frame is zeroed and filled) instead of hitting.
The full fix partitions the cache per domain (item 9, phase 1). The
minimal fix here takes away the flush:

- **Full cache.** No longer emptied. It first drops the pages whose block
  no file holds any more (`Page::is_held_elsewhere` is false: only the
  cache's own `Page` holds it). Nobody can map those again, since the
  cache holds the block and no file can hand out its key, so dropping them
  shows nothing to anyone. If still full, the page is not kept: the caller
  maps a copy of its own (`Share::Own`), taken from the cell's frames and
  counted in its quota. So no cell can push out a page another cell might
  map. The search for dead pages costs a pass over the cache; while the
  cache stays full it runs at most once every `limit / 16` (4,096)
  refusals.
- **Memory pressure.** `drop_images` calls `PageCache::trim`, which drops
  only the pages whose dropping frees memory: those no mapping uses (the
  frame's reference is the cache's alone) and those no file holds. A page
  a cell maps stays kept: dropping it freed nothing anyway, since the
  mapping keeps the frame. Pressure still drops pages no cell maps right
  now, so a cell that can exhaust memory still learns whether a page was
  mapped *since* it last forced pressure, at the price of exhausting all
  of memory per probe; reserved budgets (phase 1) take that away.

Also, a page whose frame cannot count one more reference (finding S14) is
now the caller's own copy from its own frames, counted in its quota; it
was a copy taken from the kernel's frames before.

Lost: a cache full of pages files still hold stops sharing new pages
until memory pressure or those files drop them (before, it started over).
Workloads in the benchmarks map well under 65,536 pages.

`PageCache::clear` becomes `trim`; tests that emptied the cache after
their cells ended call `trim`, which then drops every page, as `clear`
did.

## D4. What else the inventory lists

From D12 of `add-cell-hierarchy`, items marked for elimination that are
not done here, and why:

| Item | Why not now |
|---|---|
| 19, 20 Cell and thread identifiers | Only the host creates cells today, so no cell can write the counter; per-domain numbering needs domains, changes the spec of forks (a fork gets a new `getpid`) and the snapshot format |
| 1, 2, 3 Loopback and card ports as one namespace | Per-tree loopback networks and NAT (D5, D6 there), phase 1 |
| 5, 7, 8, 24, 29 Global pools and their error codes | Budgets per domain (D4 there), phase 1 |
| 9 (deduplication half) Hits on pages another cell maps | Partition per domain, phase 1 |
| 12 Sole owner of a template's frame | Templates held across domains, phase 1 |

## D5. Tests

Each leak gets a hostile in-kernel test with two cells (`observer` in its
four modes, and `hog`), failing before the fix and passing after, on both
architectures, and host tests where the logic lives in a library:

| Leak | In the kernel | On the host |
|---|---|---|
| S24 `sysinfo` | `sysinfo_shows_a_cell_only_itself`: an observer with a 64 MiB quota reads `sysinfo` before and while a neighbour holds 32 MiB: `total=64 change=0 procs=1` (before: the machine's total, `change=32`) | `sysinfo_shows_only_the_cell_itself` (`rhizo-abi`) |
| S25 ports | `ephemeral_ports_do_not_show_other_cells_binds`: a neighbour binds 100 ports between the observer's two binds: `ports +1` (before: `+102`) | `a_cells_ephemeral_ports_do_not_show_other_cells_binds`, `a_cells_ephemeral_ports_skip_ports_in_use` (`rhizo-net`) |
| S26 page cache | `memory_pressure_keeps_the_pages_other_cells_map`: `hog` exhausts memory while `observer page` maps a page of its own file: the page stays kept (before: the cache was empty) | `a_full_cache_keeps_what_others_map`, `a_full_cache_drops_pages_nobody_can_map` (`rhizo-abi`), `a_page_knows_whether_its_block_is_held_elsewhere` (`rhizo-fs`) |

One existing test asserted the leak itself: `a_full_cache_starts_over`
checked that the page past the limit emptied the cache. It is replaced by
`a_full_cache_keeps_what_others_map` (rule 9: the expected behavior
changed, in this change's `cells` delta).

## D6. Benchmarks

The `-icount` gates on the paths this touches: `cell.syscall_insns`
(`getpid`: no socket, nothing added), `cell.loopback_rtt_insns` and
`cell.tcp_resume_insns` (socket calls: the cursor's two copies),
`cell.spawn_insns` and `cell.fork_insns` (one more 8-byte field in a new
cell's record). Checked with `cargo xtask bench --icount-only --env ci
--check` on x86_64 and `--env ci-arm` on AArch64.
