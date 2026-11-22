# Proposal

## Why

The timing gate on main (issue #121) has failed on `density.sched_p99_us`
and `density.wake_p99_us` since the cell hierarchy landed (#131, `c6c8ca4`;
#132, `89c1cee`): on QEMU `microvm` the scheduling p99 went from 0.48 to
1.44 ms in the 17 `bench` runs before it to 4.5, 5.5 and 7.3 ms in the
three after it (`89c1cee`, `f5094a1`, `d211ad7`; baseline 1.90 ms, limit
3.80 ms), and the wake-up p99 likewise (0.56 to 1.70 ms, then 4.7, 5.8
and 7.3 ms; limit 4.46 ms). `q35` and Cloud Hypervisor moved the same
way and fail too on `d211ad7`. Every runner type shows it, and the lazy
resume of a worker in the same breakdown went from 2.8 to 4.8 µs to 19
to 23 µs.

The cause is the ownership tree (`rhizo_cap::Tree`): its nodes are a
vector kept sorted by identifier, and a cell that ends is removed with
`Vec::remove`, which moves every node after it; a resume (`rekey`) moves
the node to the end with a rotation over every node between. With the
server workload's 10,000 workers both are O(n) memory moves of up to
640 KB. The workers end within about 100 ms of each other at the end of
the load, so 10,000 removals in increasing identifier order (88 ms on the
host, measured with a scratch benchmark) run while the other workers'
deadlines pass, and those deadlines, more than 1 % of the samples, wait
behind them.

## What Changes

- A cell that leaves the tree leaves a gone node in its place, which
  keeps its identifier for the binary search and holds nothing; gone nodes
  are dropped in one pass once they outnumber the cells, so an end costs
  an amortized constant. A resume moves the node to its new place (the end
  of the vector, the kernel's identifiers being fresh) and leaves a gone
  node in the old one. Where at most 16 nodes would move (small trees), or
  without room, both close the gap at once, as before.
- Lookups, iteration, the roots, the count and the invariant check skip
  gone nodes; trees compare by the cells they hold.
- Tests: a host test of ends, resumes and returning identifiers among
  100 cells, through compaction; the property tests run unchanged.

### Out of scope

The other metrics of issue #121 (`cell.fork_with_files_us`, the agent
turns, `laptop.resume_turn_us` and the control path on Cloud Hypervisor):
they failed before the hierarchy, on one class of runner, and are analysed
in the pull request; no code changes for them.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `scheduling`: the tree's bookkeeping for a cell that ends or resumes
  does not grow with the number of cells.

## Impact

`crates/rhizo-cap/src/tree.rs` and its tests only; no kernel change, no
format or protocol change. The node stays 64 bytes. Memory: at most as
many gone nodes as cells (64 bytes each) until the next pass. Small trees
pay 30 to 55 instructions more per fork, end or resume (up to +1.3 % on
`cell.resume_insns`, within the 2 % gate; design, evidence).
