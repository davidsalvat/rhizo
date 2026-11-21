# kernel-heap Specification

## Purpose
Defines dynamic memory allocation for kernel code (boxes, vectors and other
heap-allocated values).

## Requirements

### Requirement: Kernel code can allocate dynamically
After boot setup the kernel SHALL support dynamic allocation and deallocation
of memory of any size up to the available physical memory, so standard
collection types can be used in kernel code.

#### Scenario: Box and Vec
- **WHEN** kernel code creates a boxed value and grows a vector to many thousands of elements
- **THEN** the values read back correctly

### Requirement: Alignment is honored
Every allocation SHALL be aligned to at least the alignment requested, for
alignments up to 4096 bytes. Requests for larger alignments SHALL fail as
out-of-memory rather than return misaligned memory.

#### Scenario: Aligned allocations
- **WHEN** allocations with alignments of 1, 8, 64 and 4096 bytes are made
- **THEN** each returned address is a multiple of its requested alignment

### Requirement: Allocations never overlap and memory is reused
Live allocations SHALL never overlap, and memory released by deallocation SHALL
be available to later allocations.

#### Scenario: Reuse after free
- **WHEN** an allocation is freed and an allocation of the same size is made
- **THEN** the allocation succeeds without consuming additional physical memory

### Requirement: Out-of-memory is reported
When an allocation cannot be satisfied, the kernel SHALL panic with a message
that includes the requested size.

#### Scenario: Impossible allocation
- **WHEN** kernel code requests more memory than the machine has
- **THEN** the kernel panics with a message containing the requested size in bytes

### Requirement: Free heap pages go back
The kernel heap SHALL give back to the frame allocator each page of its
small-block classes whose blocks are all free, once its class pages hold
at least 1 MiB and its live allocations have fallen to half the most it
counted since it last did, checked when a cell ends; allocating and
freeing a block SHALL do no more work than before.

#### Scenario: Memory back after a spike
- **WHEN** idle cells are forked until memory runs out, so the heap grows by several MiB for their kernel records, and every cell then ends
- **THEN** the heap holds at most 2 MiB more than before the cells were forked

#### Scenario: A page with a live block stays
- **WHEN** every block of a page but one is freed and the heap trims
- **THEN** that page is kept, its live block unchanged, and its free blocks are reused before any new page

### Requirement: Window page tables go back after every CPU flushed
Large allocations mapped from single frames SHALL give back the page
tables they leave empty along with their frames. Such a table's frame
SHALL be freed only once every CPU has flushed its translations and
paging-structure caches, so no CPU can walk through it after it is
reused.

#### Scenario: Fragmented large allocation
- **WHEN** a large allocation is mapped from single frames because no contiguous run is free, and is then freed
- **THEN** every frame, the page tables it needed included, is back in the frame allocator

#### Scenario: Every CPU flushes before a table is freed
- **WHEN** a freed table waits for the CPUs to flush, between host runs on 4 CPUs
- **THEN** every CPU flushes and the table is freed; while another CPU is busy, nothing is flushed and the table waits
