# Design

## D1. A reserve, checked by a counter

The frame allocator already counts free frames. Frames for cells go through
`KernelFrames`, which refuses an allocation that would leave fewer than
`RESERVE` frames free (8 MiB, or 1/8 of memory if smaller). The kernel heap
draws from the same allocator without the check, so the reserve is what
the kernel's own structures grow into. The check is one compare on a
counter, so allocation paths stay as fast.

## D2. Heap-backed cell data asks first

File blocks and snapshots live in the kernel heap, not in cell frames.
`rhizo-abi` asks `Services::can_store(bytes)` before writing a file piece
(the kernel answers from the same counter and reserve) and returns
`ENOSPC` when refused. `suspend` sums the cell's pages first and returns
`None` (the cell keeps running) when the snapshot would not fit; eager
`resume` does the same with the snapshot's size and returns `None`.
A lazily resumed cell that cannot get a page later faults, like any cell
that runs out of memory.

## D3. Density, measured

`idle` prints `idle` and sleeps for an hour in a loop. Frozen as a template
and forked 1000 times, with each child run once into its sleep, the
average drop in free memory per child is `cell.idle_bytes`: a root table,
the tables and pages its first steps copy (stack), and its kernel record.
`cell.suspended_bytes` is the snapshot of one idle cell.
