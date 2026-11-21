# Design

## D1. Refer to shared frames, after making the tables private

A frame's reference count already says whether another cell can see it:
copy-on-write (design D2 of `add-fork`) keeps the invariant that an entry
pointing at a frame with more than one reference forbids writes. A
snapshot adds one more reference to each page it refers to, so the frame
stays allocated and every holder that writes it copies it first.

Pages are not the only thing a fork shares: `fork_lower_half` shares the
child's page tables too, and a page below a shared table has one
reference (one table points at it), protected only by the table entry
above. Once the table stops being shared (the other cell ended, or copied
its path), its owner writes that page in place. Referring to it as it is
would let the template change a suspended agent's memory. So `capture`
first calls `PageTable::unshare_tables`: every shared table of the cell
is copied exactly as a write fault would (`walk_private`, now sharing
its step `private_table`), which marks the pages below copy-on-write in
both copies and counts both references. Then "shared" is just
`is_shared(frame)`, and `try_share` takes the snapshot's reference (a
full count copies the page instead, as in `fix-shared-frame-count`).

| Option | Memory per suspended agent | Cost |
|---|---|---|
| **Refer to shared frames, tables made private first (chosen)** | its own pages (82 KB on the laptop) | one walk of its tables, a copy of each shared table |
| Copy every page (today) | every page (1.28 MB) | a copy of every page |
| Refer to the template's tables as they are | less | a table shared with a running cell would have to be write-protected for good, or pages below it change |
| Keep the suspended cell's page tables alive instead of a snapshot | its tables | the snapshot would not be a byte form any more: no export, no validation |

The suspending cell is destroyed right after, so its private tables live
for one call. Freeing them walks each page (one count down each) instead
of dropping shared tables whole: still far cheaper than the copies it
replaces.

Capture runs under the lock of shared tables (`SHARING`) when the cell
shares memory, since copying a shared table changes entries other cells
see, exactly as `walk_private` does under that lock. Releasing a
reference takes no such lock: it only lowers a count (under the lock of
the counts), and a cell that then sees the frame unshared is its only
holder. A resumed cell maps referred frames in tables of its own, so it
shares only pages, never tables: like a cell mapping a loaded image's
pages, it needs no `SHARING` either.

## D2. Exports stand alone; the format does not change

The references are physical addresses, meaningful only in the kernel that
took them and only while the snapshot holds them. A snapshot leaving the
kernel can either carry the pages' contents or name what it depends on.

| Option | Format | Needs |
|---|---|---|
| **Self-contained export (chosen)** | version 7 as today | the frames' contents read once at export |
| Dependency on the template's own snapshot | new version: a template identity and per-page "from the template" records | snapshot identities, shipping and validating both together, a template snapshot that matches the live template's frames |

Nothing exports snapshots across kernels yet (the control plane keeps
them in memory, tests round-trip them); the dependency would be design
for a use that does not exist. So the in-kernel form marks a record that
refers to a frame with 1 in its first reserved byte and puts the physical
address in its offset field, with no data; `export` rewrites those
records and writes the contents in their place, producing exactly what a
copying capture produced. `from_bytes` already requires the reserved
bytes to be zero, so a snapshot from outside can never name a physical
frame (spec `isolation`). The version stays 7, and no number is taken
from the queue of format changes.

## D3. Releasing references

A `Snapshot` is plain data in a library crate, but it now holds frames.
Dropping one without giving them back would leak memory, and snapshots
are dropped in many places (a killed suspended cell in the control plane,
the end of a lazy resume, tests). So the snapshot keeps the function that
releases a reference (`Release = fn(u64, PageSize)`, the kernel's frames
in the kernel) and calls it for each referred frame when dropped. A
snapshot from `from_bytes` refers to nothing. `Clone` goes away: a clone
would release the same references twice.

## D4. Restore

`restore_page` maps a referred frame with one more reference, its
permissions kept, writable pages copy-on-write (`PageTable::map` now
honours `COPY_ON_WRITE`, as `protect` does). A lazily resumed cell holds
the snapshot until its last page is in, then drops it: a frame the cell
never wrote ends with the cell's reference alone and is written in place
from then on. Resuming eagerly needs frames only for the pages the
snapshot holds itself, which is what memory accounting now asks for.

## Measurements

QEMU TCG, laptop workload (`cargo xtask bench --workload laptop`), one run:

| Metric | AArch64 before | after | x86_64 (microvm) before | after |
|---|---|---|---|---|
| `laptop.memory_bytes` | 61,095,936 | 7,462,912 | 60,424,192 | 6,606,848 |
| snapshot bytes (heap) | 1,273,841 | 81,905 | 1,277,961 | 81,929 |
| `laptop.suspend_us` | 1688 | 347 | 1401 | 289 |
