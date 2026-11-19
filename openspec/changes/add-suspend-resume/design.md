# Design

## Context

Cells are plain data (`Cell`: registers, root, break, mapping cursor); their
memory is whatever the user half of their page table maps. H3 keeps live
cells in a scheduler; a cell that is not running has all its state saved.

## Decisions

### D1. What a snapshot is

A snapshot holds the cell's plain state and a copy of every mapped page
(address, size, permissions, contents). Suspending copies the pages and
returns every frame and table to the allocator, so a suspended cell costs
only its bytes. Rejected: keeping the frames and dropping only the page
tables. It is faster to suspend but frees almost nothing, and it is not a
snapshot that can be stored or moved. The copy is the honest baseline;
copy-on-write sharing of frames comes with fork in H5.

### D2. Format

```
header   magic "RHZSNAP1", format version (u32), ELF machine (u16),
         page count (u32), flags
cell     pc, sp, x0-x30 / gpr, tls, flags, brk_start, brk, mmap_next,
         sleep_remaining_ns (u64, 0 = runnable)
records  per page: virtual address (u64), size (4 KiB or 2 MiB), permissions
data     page contents, in record order
```

Every field is written explicitly in little endian, so the format does not
depend on the memory layout of Rust types; records are sorted by address,
so encoding the same cell twice gives the same bytes. Decoding checks the
magic, version, machine, record count against the data length, user-half
addresses and alignment, and fails with an error that names the problem.

### D3. Resume modes

| Mode | When the cell can run | Cost |
|---|---|---|
| Eager | after every page is mapped and copied | proportional to memory |
| **Lazy (default)** | right away: only an empty root and the registers | per page, on first touch |

Lazy resume keeps the snapshot with the cell until every page is mapped. A
fault from cell code at an address of a pending page maps it and retries
the instruction; any other fault kills the cell as before. System calls go
through a pager hook in `rhizo-cell` memory access, so a `write` from a
buffer that is not back yet pages it in instead of returning `EFAULT`.

### D4. Equivalence

A resumed cell must be indistinguishable from one that never stopped: same
registers, same memory, same break and mapping cursor, same remaining sleep.
The tests compare full program output between an uninterrupted run and runs
suspended and resumed after every scheduling step, in both modes. The test
programs avoid wall-clock time and randomness, whose values legitimately
differ between runs.

### D5. Identity

A resumed cell gets a new identifier and a new address-space tag; `getpid`
inside it returns the new identifier. Keeping identifiers stable
across hosts is a later concern (migration).

## Risks / Trade-offs

- [Lazy resume of a cell that never touches some pages keeps the snapshot
  alive] → the snapshot is dropped when the last page is mapped or the cell
  ends; a later version can page in the rest in the background.
- [Large pages: lazy fault copies 2 MiB at once] → accepted; the cell asked
  for 2 MiB mappings.
