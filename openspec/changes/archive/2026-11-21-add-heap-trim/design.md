# Design

## D1. Find wholly free pages by sorting the free lists

A class's free list is singly linked through the free blocks themselves,
in no order. A page can go back only when all of its blocks are free, so
the heap must know, per page, how many of its blocks are free.

| Option | Hot path | Memory | Trim |
|---|---|---|---|
| **Sort the free list by address at trim time (chosen)** | unchanged | none | O(F log F) for F free blocks |
| Per-page counts in a side table, doubly linked free lists (a slab allocator) | a count and two more links on every alloc and free | a radix table of counts | O(blocks of the page) when a page empties |
| A header in each page | as above | one block per page: a quarter of the 1024-byte class, half of the 2048-byte one | as above |

Sorting needs no memory and changes nothing for allocating and freeing.
Merge sort of a linked list in place (runs of 1, 2, 4… blocks merged in
pairs, by relinking) costs O(F log F). Once sorted, a page's free blocks
are adjacent: a walk counts each run, gives back the pages whose run is
complete, and relinks the rest in address order, which later allocations
then follow.

## D2. When to trim: at cell ends, on a cold condition

The first attempt checked a byte counter in every `dealloc` (trim when
the free blocks reach twice what the last trim left). Under `-icount`,
that counter alone added 0.2 to 0.7 % to `cell.file_write_read`,
`cell.loopback_rtt` and `cell.tcp_resume` on x86_64. So the heap does no
accounting of its own on the hot path, and its owner asks it to trim at
moments when many allocations may have gone:

- `Heap::trim_if_due` notes the most live allocations it has seen since
  the last trim (`stats.live` is already counted) and trims when the
  class pages hold at least `TRIM_AT` (1 MiB) and the live allocations
  fell to half that most. Two comparisons when not due.
- The kernel calls it at the end of `cell::end`, once the cell's record
  is dropped: a cell ending is when the kernel frees most at once.

Cost: a trim needs the live allocations to halve since the previous one,
so the frees between two trims number at least half the allocations
alive at the last trim. A trim leaves free blocks only in pages that hold
a live block, at most 256 per such page. With N cells ending, at most
about log2(N) trims run. The heap lock is held during a trim; in the
server workload's out-of-memory push (46,000 cells) the trims stay below
the noise of the runs' timings.

What is left after the last cells end is what was freed since the last
trim, at most what the heap's own live allocations at that point
amount to: 0 bytes in both server runs, under 2 MiB in the in-kernel
test, where memory is 128 MiB.

## D3. Free the window's empty page tables

The window maps single frames at addresses never handed out twice, so a
stale translation on any CPU can never reach memory freed there. Its
page tables were never freed: each 2 MiB of addresses used kept a 4 KiB
table for good. `PageTable::free_empty_tables` frees the last-level
tables (2 MiB) and the level above (1 GiB) that a freed range leaves
empty, clearing their entries and invalidating them on this CPU
(`tlbi vaae1is` on AArch64, `invlpg` on x86_64). The table below the
root stays: on x86_64, cell roots copy the kernel's top-level entries,
which point at it.

## D5. Free a table only after every CPU flushed

Addresses that are never reused make stale *translations* harmless, but
not a freed *table frame*. Once the frame is reused as data, a CPU that
still caches an upper entry pointing at it may walk through it
speculatively or by prefetch, reading garbage as entries. On x86_64,
`invlpg` only reaches this CPU and the current PCID, and kernel-half
walks happen under every cell's PCID. So an emptied table is retired, not
freed:

- **Retired list.** The tables go on a list linked through their first
  entry. A frame address is 4 KiB-aligned, so it is never a valid entry,
  and a walk through a retired table finds nothing. The list changes only
  under the heap's lock.
- **Full flush per CPU.** `cpu::flush_all` drops every translation and
  paging-structure cache entry of the CPU, for every PCID, global ones
  included: on x86_64 a write to CR4 that toggles PGE (set back at once);
  on AArch64 `TLBI VMALLE1IS` between `DSB` barriers, broadcast to every
  core.
- **Every CPU.** `smp::flush_everywhere` runs it on every CPU and frees
  nothing itself. On x86_64 with several CPUs, the boot CPU posts it to
  each secondary through the mailbox and waits for all of them. It does
  this only when every secondary is idle; otherwise, or when it runs on
  a secondary, it returns `false` and does nothing. On AArch64 this CPU's
  broadcast flush is enough.
- **When reclaim happens.** Right after the window unmaps an allocation
  (frees happen under the heap's lock), and at the end of `Host::run`,
  once the other CPUs are back. Tables retired during a run, when the
  other CPUs are busy, wait for its end. This is rare: only a free that
  empties a table, through a window used only when no contiguous run of
  frames is free.

There is **no IPI shootdown**. The caller holds the heap's lock, and the
kernel runs with interrupts masked: interrupts arrive only while cell
code runs or a CPU idles. Another CPU may be spinning on that very lock
in kernel code, and would never take an interrupt, so the caller would
wait forever. Deferring the free to a moment when every CPU can be
reached is what an OS does with page tables anyway: free them after the
flush, not before.

## D4. Tests that count frames

Several in-kernel tests ran a round to warm the heap, then checked that
`memory::free_bytes()` came back to the same value after a second round:
they relied on the heap keeping every page it grew into. Now a cell's end
may trim the heap. Whether it does depends on what ran before, and with
several CPUs on the order in which cells end, so one of these tests
failed on 4 CPUs in CI. Those tests now compare the free frames plus the
frames the heap holds (`free_or_in_heap`). A trim does not change that
sum, and a leak still does. The check stays an exact equality.

For the same reason, `cell.idle_bytes` (a thousand idle forks, measured
in a second round after a first that warmed the heap) now includes each
cell's kernel record: the first round's cells end, the heap gives their
pages back, and the second round grows into new ones. That cost was real
before as well, but the warm-up hid it.

## Measurements

QEMU TCG, `cargo xtask bench --workload server --runs 1` (base: `main`
at `c479b0d`):

| | x86_64 (microvm) before | after | AArch64 before | after |
|---|---|---|---|---|
| `heap_kept_bytes` after the out-of-memory push | 75,018,240 | 0 | 75,018,240 | 0 |
| `heap_peak_bytes` (heap growth while memory was full) | – | 93,855,744 | – | 93,622,272 |
| `density.oom_cells` | 46,760 | 46,878 | 46,648 | 46,923 |

x86_64 cell figures (`--no-reference`, one run): `cell.idle_bytes`
20,516 to 21,573 (D4); every other size and frame count unchanged.
