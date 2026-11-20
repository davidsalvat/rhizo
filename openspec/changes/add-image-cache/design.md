# Design

## D1. Load once, start many

`rhizo_cell::load(image)` maps the segments into a new hierarchy and
nothing else; `rhizo_cell::start(id, image, pt, …)` shares the kernel half
when the architecture needs it, builds the stack and the first thread.
`spawn` is `load` then `start`, so the loader exists once. The kernel keeps
the hierarchy `load` returns for each executable it spawned (never run, no
kernel half, no stack) and starts every later cell of that executable on a
copy of it (D2). Nothing about the cell differs from a loaded one: same
registers, stack, break and page contents (host test).

## D2. The copy has its own tables, shares read-only pages, copies the rest

`PageTable::clone_lower_half` walks the kept hierarchy and builds new
tables: a read-only leaf is shared (`Frames::share`), a writable (or
copy-on-write) leaf gets a private copy of its page, writable at once.

Alternatives that lost:

- **`fork_lower_half`, sharing tables** (as `spawn_from_template` does).
  Allocates one root per spawn, but the cell then shares tables, so it is
  marked as sharing and every system call and write fault of every cell
  takes the `SHARING` lock (design D6 of `add-smp-scheduling`): cells that
  have nothing to do with each other would serialize on it across CPUs.
  Dropping a kept space would also need `SHARING` from wherever frames run
  short, including system calls that hold the sockets' lock, which take
  `SHARING` first: a lock-order inversion.
- **Sharing writable pages copy-on-write too.** Cheapest spawn, most memory
  shared, but CPython writes most of its 1.3 MB of data and `.bss` while
  it starts, so it would take a fault and copy per page instead of a copy
  at spawn. Copying at spawn costs the same copies without the faults and
  starts the cell exactly as a load does.

The copy only reads the kept tables and changes reference counts, which
are atomic per page (`SHARED`). A cell started on a copy shares no table,
so it is not marked as sharing: a write to a page it shares never happens
in place (a shared read-only page made writable by `mprotect` is
copy-on-write, as after a fork), and a page is only written in place by
its last holder, which no one else can start holding (the kept space holds
no page that a cell holds alone).

## D3. Which executables are kept

By the image's address and length: kernel images are `&'static` (the
`Host::spawn` signature now says so), so the pair names one executable for
as long as the kernel runs. Four are kept, the least recently spawned
dropped first (`rhizo_cell::images::Images`, a fixed array: no heap
allocation under the lock). The `bench-cell` kernel and the tests spawn a
handful of programs in turns, and CPython, the largest, stays kept while it
is measured.

## D4. Memory

A kept space takes its frames through `memory::alloc_for_cell`, like a
cell: never from the kernel's reserve. It belongs to no cell, so no cell
is charged for it. Frames for cells come first:

- `KernelFrames::alloc` (every frame a cell gets) and the cells' memory
  check (`cells_may_take`: `brk`, `mmap`, files, stack growth, suspend,
  eager resume) drop every kept executable and try again before saying no.
- Allocations made under the image lock (copying a kept space) use
  `ImageFrames`, which never drops, so the lock is never taken twice on one
  CPU. If such a copy runs out of frames, the spawn takes the kept space
  itself (out of the cache) and every other kept executable is dropped: a
  spawn never needs more memory than a load.
- `alloc_large` does not drop: a failed 2 MiB allocation may only mean
  fragmentation, and its callers fall back to 4 KiB pages, which do.

Loading a missing executable happens outside the lock, so it may drop the
others; if another CPU loaded the same one meanwhile, the second load is
destroyed. Dropping frees only what no cell still shares (reference
counts), so it is safe while cells of that executable run on other CPUs.

In-kernel tests that compare free memory before and after cells
(`memory_balance`, `cells_in_parallel_stay_isolated`) drop the kept
executables before each measurement: their frames are the kernel's until
then (spec `cells`).

## D5. Instruction cache

On AArch64 the executable pages of a kept space are synchronized with
instruction fetch once, after loading and before the first copy. Copies map
the same physical frames at the same addresses, read-only, and nothing ever
writes them again (writes only reach private copies, D2), so the
synchronization still holds for every copy, as Linux synchronizes a page
cache page once, not per mapping. A frame is synchronized again only when
it is loaded into anew after being freed.

## D6. Benchmarks

`cell.spawn_us` (the `hello` spawn) and `cell.run_us` measure the copy
instead of the load from the second of their 101 runs on, and
`workload.cpython_cold_us` from its second round. No metric is added: the
existing ones isolate the effect (spawn alone, spawn to exit, and the
interpreter's cold start). The cold figure stays a start from the
executable: the Linux process it is compared with is also started from an
executable its kernel keeps in memory (the page cache).
