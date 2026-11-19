# Design

## Decisions

### D1. A persistent tree, shared by reference counting

The file system of a cell is an inode table whose entries are
reference-counted inodes (`Rc`); a directory maps names to inode numbers, a
regular file is a size and a list of reference-counted 4 KiB blocks. Cloning
the table for a fork costs one count per inode, not per byte; a write makes
private only what it touches (`Rc::make_mut`): the inode, its block list and
the written block. The kernel runs on one CPU, so `Rc` needs no atomics.

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
with the snapshot's `BadFiles` error. `rhizo-snapshot` treats the section as
opaque bytes; `rhizo-fs` encodes and decodes it.

### D4. Robust cell figures

Each cell figure printed by a `bench-cell` kernel becomes the minimum over
five rounds of the median of its round, instead of one median: the
operation's cost, not the noise of the slowest moment. Thresholds and
baselines are unchanged.
