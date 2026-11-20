# Spec Delta

## ADDED Requirements

### Requirement: Cells isolated on every CPU
Cells running at the same time on different CPUs SHALL stay isolated from
each other exactly as on one CPU: each sees only its own memory, files,
registers and FP/SIMD state, and every frame and tag returns when they
end. A cell's FP/SIMD state and address space SHALL leave a CPU with the
cell, so the next CPU to run it continues it unchanged.

#### Scenario: Many programs at once
- **WHEN** four copies each of `counter`, `filer`, `libc_threads`, `chatter` and `threads` run at once on 4 vCPUs
- **THEN** each prints exactly what it prints alone and exits with 0, and all memory comes back

### Requirement: TLB coherence across CPUs
No CPU SHALL use a stale translation of a cell: one that an address-space
change of the cell (unmap, protection change, copy-on-write, fork) removed
or narrowed, or one that a previous holder of the cell's tag left. Where
invalidation is per CPU (x86_64), a CPU SHALL drop the cell's tag before it
next runs the cell if the cell's translations shrank, or the cell got its
tag, since that CPU last dropped it.

#### Scenario: Copy-on-write on several CPUs
- **WHEN** forks of a cell and templates run, write and exit on 4 vCPUs
- **THEN** each sees only its own writes, as on one CPU

### Requirement: CPUs let go of the cells they give back
A CPU SHALL no longer use a cell's tables or hold its FP/SIMD state once it
has given the cell back, so another CPU can run the cell, or free it, at
once.

#### Scenario: A cell that moves
- **WHEN** two cells computing with different FP/SIMD values yield to each other on 4 vCPUs, each moving between CPUs
- **THEN** each gets its exact result

### Requirement: Cells stopped before host operations
Suspend, resume, fork, freeze, kill and spawn SHALL act on cells none of
whose threads runs on any CPU: they happen between runs, after every CPU
gave back its cell.

#### Scenario: Suspend between parallel runs
- **WHEN** `counter` runs next to spinning cells on 4 vCPUs, and is suspended and resumed from its snapshot's bytes after every few slices until it exits
- **THEN** its whole output is the uninterrupted one

### Requirement: Shared copy-on-write memory changes one CPU at a time
Changes to page tables and frames that cells share copy-on-write (resolving
a write, unmapping, changing protections, freeing) SHALL be made by one CPU
at a time, so reference counts stay exact; cells that share no memory SHALL
not wait for each other.

#### Scenario: Children of one parent at once
- **WHEN** children forked from one cell write to and free shared pages on several CPUs at the same time, then exit
- **THEN** every shared frame and table is returned

### Requirement: Kernel allocations survive fragmentation
A large kernel heap allocation SHALL succeed whenever enough frames are
free, contiguous or not.

#### Scenario: No two free frames together
- **WHEN** every other free frame is taken and the kernel allocates 1 MiB
- **THEN** the allocation succeeds, holds what is written, and its frames come back when it is freed
