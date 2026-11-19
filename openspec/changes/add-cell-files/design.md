# Design

## Decisions

### D1. A persistent tree, shared by reference counting

The file system of a cell is an inode table whose entries are
reference-counted inodes (`Arc`); a directory maps names to inode numbers, a
regular file is a size and a list of reference-counted 4 KiB blocks. Cloning
the table for a fork costs one count per inode, not per byte; a write makes
private only what it touches (`Arc::make_mut`): the inode, its block list and
the written block. `Arc` rather than `Rc` because kernel globals hold cells
and must be `Send`; on one CPU its atomic counts are uncontended.

The tables themselves (inode table, descriptions, descriptors, working
directory) sit behind one more `Arc`, copied on the first change: a fork is
one count, and every cell spawned or resumed without files shares a single
empty file system built once, so neither path builds a table (the lazy
resume benchmark showed the cost of building one).

| Option | Fork cost | Write after fork |
|---|---|---|
| **Shared inodes and blocks (chosen)** | one count per inode | copy one inode, its block list, one block |
| Deep copy | all bytes | none |
| Shared store with explicit counts (as page frames) | one count per block | as chosen, more code |

### D2. Linux semantics, as far as cells need

Descriptors point at open file descriptions (offset, access mode, append
flag) shared by `dup`, as on Linux; descriptors 0–2 are the console. A file
unlinked while open keeps its data until its last description closes. Paths
are resolved from the root, the working directory or a directory
descriptor; `.` and `..` work; there are no links. Modes are fixed (files
0644, directories 0755); `O_DIRECTORY` differs between architectures and is
decoded per architecture.

### D3. Snapshot format version 3

The encoded file system follows the page data: a length, then the inode
table (kind, link count, directory entries or file bytes), the open
descriptions and descriptors, and the working directory, all little endian.
Decoding validates every reference (inode numbers, descriptions) and fails
with the snapshot's `BadFiles` error. `rhizo-fs` encodes and decodes the
section; `rhizo-snapshot` depends on it so that `from_bytes` refuses a
snapshot whose files do not decode, as it refuses a bad page record.

### D4. Robust cell figures

Each cell figure the `bench-cell` kernel repeats a hundred times or more
(`cell.spawn_us`, `cell.run_us`, `cell.spawn_from_template_us`) and the new
file figures become the minimum over five consecutive rounds of the median
of each round, instead of one median: the operation's cost, not the noise
of the slowest moment. Figures of seven repetitions (large forks, suspend,
resume, workloads) stay medians: five rounds of one or two samples would be
a minimum, not a median. Thresholds and baselines are unchanged.

### D5. Bounded file data

A cell holds at most 64 MiB of file data (`ENOSPC` beyond, `EFBIG` for a
file larger than that). File blocks live in the kernel heap, which panics
when memory runs out, so the bound keeps one cell from exhausting it. Many
cells together can still exceed memory; accounting against free frames is
left to a later change.
