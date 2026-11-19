# Spec Delta

## Purpose

Defines dynamic memory allocation for kernel code (boxes, vectors and other
heap-allocated values).

## ADDED Requirements

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
