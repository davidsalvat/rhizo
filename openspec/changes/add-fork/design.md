# Design

## Context

A cell's memory is the user half of its page table: four levels, leaves of
4 KiB or 2 MiB. Frames come from the bitmap allocator and belong to exactly
one cell. H4 copies every page when suspending.

## Decisions

### D1. Reference counts

A dense array of 16-bit counts, one per frame of RAM, created on the first
fork (64 KiB for 128 MiB of RAM). A frame with no entry counts as 1. `free`
decrements and releases the frame at zero. Large pages count on their first
frame. Rejected: a map of shared frames (a fork of 50 MiB would insert
thousands of keys).

### D2. Copy-on-write that shares page tables

| Option | Fork cost | Write-fault cost |
|---|---|---|
| Copy the page tables, share leaves (classic) | proportional to mapped pages | copy one page |
| **Share the tables too (chosen)** | the root's 256 lower entries | copy the tables on the path (at most 3), then the page |

Fork: for every present lower-half root entry of the parent, clear the
table-level write permission (x86_64 `W` in the table entry, AArch64
`APTable[1]`), copy the entry into the child's new root, and count a second
reference to that table. Both cells now read the same tables; neither can
write through them. Because the parent's permissions shrank, its TLB tag is
flushed.

A write fault walks from the root. At a read-only table entry: if the table
has other references, copy it, and in the copy and the original mark every
table entry read-only and every writable leaf read-only with a
copy-on-write mark (a software bit), adding a reference to each child table
and frame; point this cell's entry at the copy, writable. If this cell holds
the only reference, make the entry writable. At the leaf: a copy-on-write
leaf whose frame has other references gets a private copy; with the only
reference it becomes writable again. A read-only leaf without the mark is a
real permission fault.

### D3. Effective permissions

A page is writable only if its leaf and every table above it allow writes.
`translate` reports the effective permissions, so `copy_to` from a system
call fails on a shared page and the pager resolves it like a cell's own
write fault. Mapping or unmapping in a cell (`brk`, `mmap`, `munmap`)
first makes the tables on the path private.

### D4. Templates

A template is a cell that never runs: spawned (or run to a ready point) and
then frozen. `spawn_from_template` forks it. Its cost is a fork, not ELF
loading. In H6 a warm interpreter becomes a template.

### D5. Fork semantics

Children are identical to the parent except for their identifier (and so
`getpid`) and address-space tag. Forking a lazily resumed cell first
restores its pending pages.

## Risks / Trade-offs

- [Subtle sharing bugs break isolation] → host property tests drive random
  forks, writes, maps, unmaps and frees against a model of each cell's
  memory, on both entry formats; in-kernel tests check every cell sees only
  its own writes and that all frames return.
- [16-bit counts overflow past 65535 sharers] → forks fail beyond it.
