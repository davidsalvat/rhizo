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
empty, clearing their entries and invalidating them (`tlbi vaae1is` on
AArch64 drops every level for the address; `invlpg` on x86_64 drops the
paging-structure caches for it). The table below the root stays: on
x86_64, cell roots copy the kernel's top-level entries, which point at
it. Other x86_64 CPUs are not shot down. As for the pages themselves,
nothing ever translates those addresses again, so a cached entry
pointing at a freed table is never used.

## Measurements

QEMU TCG, `cargo xtask bench --workload server --runs 1` (base: `main`
at `c479b0d`):

| | x86_64 (microvm) before | after | AArch64 before | after |
|---|---|---|---|---|
| `heap_kept_bytes` after the out-of-memory push | 75,018,240 | 0 | 75,018,240 | 0 |
| `heap_peak_bytes` (heap growth while memory was full) | – | 93,855,744 | – | 93,622,272 |
| `density.oom_cells` | 46,760 | 46,878 | 46,648 | 46,923 |
