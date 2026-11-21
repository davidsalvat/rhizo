## ADDED Requirements

### Requirement: Free heap pages go back
The kernel heap SHALL give back to the frame allocator each page of its
small-block classes whose blocks are all free, once its class pages hold
at least 1 MiB and its live allocations have fallen to half the most it
counted since it last did, checked when a cell ends; allocating and
freeing a block SHALL do no more work than before. Large allocations
mapped from single frames SHALL give back the page tables they leave
empty along with their frames.

#### Scenario: Memory back after a spike
- **WHEN** idle cells are forked until memory runs out, so the heap grows by several MiB for their kernel records, and every cell then ends
- **THEN** the heap holds at most 2 MiB more than before the cells were forked

#### Scenario: A page with a live block stays
- **WHEN** every block of a page but one is freed and the heap trims
- **THEN** that page is kept, its live block unchanged, and its free blocks are reused before any new page

#### Scenario: Fragmented large allocation
- **WHEN** a large allocation is mapped from single frames because no contiguous run is free, and is then freed
- **THEN** every frame, the page tables it needed included, is back in the frame allocator
