# Proposal

## Why

The server reference workload (`add-reference-workloads`) ends with an
out-of-memory push: idle cells forked until the kernel refuses one, then
all of them ended. Every frame comes back, but 75 MB of it stays in the
kernel heap (`heap_kept_bytes=75026432` on QEMU, both architectures): the
pages that held the cells' kernel records, carved into small blocks, are
kept on the free lists forever (design D6 of the heap: "the heap keeps
size-class pages"). A kernel whose number of cells goes up and down holds
its peak in the heap for good, and cells cannot use that memory.

Large allocations already give their frames back, but when they come
from the window of single frames (`0xffff_9000_…`, used when no
contiguous run is free) the page tables they needed stay: addresses there
are never reused, so each 2 MiB of the window used costs a 4 KiB table
for the rest of the kernel's life.

## What Changes

- `Heap::trim`: sorts each class's free list by address (an in-place
  merge sort of the list's links, no memory) so the free blocks of one
  page are adjacent, and gives back every page whose blocks are all free.
- `Heap::trim_if_due`: trims when the class pages hold at least 1 MiB
  (`TRIM_AT`) and the live allocations fell to half the most it saw since
  the last trim. The kernel calls it when a cell ends. Allocating and
  freeing are unchanged: no counter, no check on the hot path.
- `PageTable::free_empty_tables`: frees the tables of the last two
  levels that a range leaves empty. The window calls it after unmapping
  a large allocation, so its tables come back with its frames.
- The server workload reports `heap_peak_bytes` and a new metric,
  `density.oom_heap_kept_bytes`.

## Capabilities

### Modified Capabilities

- `kernel-heap`: free pages go back to the frame allocator.

## Impact

- `crates/rhizo-mm` (`heap`: trim, sort; `paging`: `free_empty_tables`),
  `kernel/src/heap.rs`, `kernel/src/cell.rs` (`end`),
  `kernel/src/workloads.rs`, `kernel/src/tests.rs`, `xtask/src/workloads.rs`,
  `bench/README.md`.
- Server: `heap_kept_bytes` 75 MB to 0 on both architectures.
- No gated instruction count moves by 0.1 % or more; a cell's teardown
  gets two comparisons.
