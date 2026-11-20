# Proposal

## Why

Running the MicroPython benchmark before the others made every later lazy
resume about 2× slower and eager resume about 6× slower, with the snapshot
unchanged. The frame allocator is next-fit: after a large churn its search
point sits far into memory, so the next cells get frames never touched
before. Under a VMM the first touch of a guest page costs a fault on the host
(the page is backed lazily), and on any machine cold frames miss the caches.
Reusing freed frames first keeps cells on memory that is already warm.

## What Changes

- Freeing frames below the allocator's search point moves the point back to
  them, so the lowest freed frames are handed out before frames never handed
  out. One compare per free; allocation is unchanged.
- The MicroPython benchmark runs before the others again: the figures after
  it must not depend on its churn (that is the benchmark of this change).

## Capabilities

### Modified Capabilities

- `physical-memory`: freed frames are reused first.

## Impact

`crates/rhizo-mm/src/frames.rs`, `kernel/src/cell.rs` (benchmark order).
