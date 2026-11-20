# Design

## D1. Lowest freed frame first

`free_contiguous` sets the search hint to `min(hint, first freed frame)`.
`alloc`, `alloc_contiguous` and `alloc_large` keep searching forward from the
hint (word at a time), so they now find the freed frames first, then frames
past the old hint. It is a next-fit allocator that never runs ahead of free
memory it could reuse.

## D2. The benchmark that shows it

The bench-cell kernel runs the MicroPython request benchmark (a large churn
of cell memory and heap) before the suspend/resume and fork figures. Before
this change that order doubled `cell.resume_us` and slowed
`cell.resume_eager_us` six-fold; after it they match the runs without
MicroPython, which the `ci` gate checks.
