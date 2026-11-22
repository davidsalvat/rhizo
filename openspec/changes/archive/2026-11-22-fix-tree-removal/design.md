# Design

## Evidence

Every `bench` run on main from `b696efa` to `d211ad7` (21 runs, job logs),
QEMU `microvm`, medians of three boots:

| Runs | `density.sched_p99_us` | `density.wake_p99_us` | breakdown `resume_ns` |
|---|---|---|---|
| 17 before `c6c8ca4` (all runner types) | 475 to 1442 | 557 to 1704 | 2774 to 4778 |
| `89c1cee`, `f5094a1`, `d211ad7` | 4456, 5505, 7340 | 4719, 5767, 7340 | 22802, 18686, 20693 |
| `ci` baseline (limit) | 1900.5 (3801) | 2228.2 (4456) | not gated |

`q35` and Cloud Hypervisor moved by the same factor (`d211ad7` fails on
both too); Firecracker's p99 (2.5 to 5.5 ms before, from faulting in guest
memory) hides it. The p50 did not move, and the busy time of the load
grew by about 90 ms (`idle_ns` 2.41 s to 2.32 s on the same runner type):
a burst, not a slower common path.

On the host, a scratch benchmark of `rhizo_cap::Tree` with 10,004 roots
(the server workload's workers and templates):

| Operation | Before | After |
|---|---|---|
| End all 10,004, in identifier order | 88.2 ms (8.8 µs each) | 0.73 ms (73 ns each) |
| `rekey` from the middle to a fresh identifier | 6.5 µs | 0.24 µs |
| `add_root` with a fresh identifier | 91 ns | 89 ns |

In the guest, `cargo xtask --arch x86_64 bench --workload server --vmm
qemu-microvm --accel icount --runs 1` (the clock counts instructions, so
the runs compare on one host without KVM):

| Tree | `sched_p99` | `wake_p99` | `resume_ns` | `idle_ns` |
|---|---|---|---|---|
| `5a23f6d` (before the tree) | 14.2 ms | 14.2 ms | 8145 | 743 ms |
| `origin/main` (`f5094a1`), 3 runs | 159.4 ms (all 3) | 159.4 ms (all 3) | 88380 to 88592 | 489 to 491 ms |
| this change, 4 runs | 14.2 ms (all 4) | 14.2 ms (all 4) | 9101 to 9757 | 735 to 738 ms |
| PR #138 (`ee2ed83`, weighted scheduler), 2 runs | 159.4 ms | 159.4 ms | 88467, 88492 | 487, 488 ms |
| PR #138 with this change, 2 runs | 14.2 ms | 14.2 ms | 9146, 9697 | 732, 746 ms |

The weighted scheduler of #138 neither causes nor hides the regression.

What it costs a small tree, in instructions (`cargo xtask --arch x86_64
bench --icount-only`, against `origin/main` measured the same way): one
more comparison per lookup (is the node gone) and per end and resume
(how far the node would move), about 30 to 55 instructions per
operation, within the 2 % gate:

| Metric | `origin/main` | this change |
|---|---|---|
| `cell.fork_insns` | 6973 | 7010 (+0.5 %) |
| `cell.fork_with_files_insns` | 6940 | 6977 (+0.5 %) |
| `cell.spawn_from_template_insns` | 6936 | 6973 (+0.5 %) |
| `cell.resume_insns` | 4268 | 4324 (+1.3 %) |
| `cell.tcp_resume_insns` | 73988 | 74100 (+0.2 %) |

Every other instruction count is within 0.1 %. This is the price of
bounding the work per operation: at 10,000 cells the same resume costs
88 µs less (under `-icount`) and an end 8.7 µs less on the host.

## D1: Gone nodes instead of closing the gap

The nodes stay one vector sorted by identifier (one cache line each,
binary search, no allocation per node). A cell that ends leaves its node
in place with depth `GONE` (255, deeper than any cell: the node has no
spare byte for a flag and stays 64 bytes), its links cleared and its
policy reference dropped. The gone nodes keep the identifiers in order,
so the binary search still works; `index` treats a gone node as absent.
When the gone nodes outnumber the cells, one `retain` drops them all:
each end pays its share of one pass, an amortized constant. A node with
at most 16 nodes after it (1 KiB) is still removed at once, as cheap as
a gone node, so small trees, such as the benchmarks' one or two cells,
keep no gone nodes.

Lost alternatives:

- `BTreeMap<u32, Node>`: O(log n), but its insertions allocate and abort
  on failure, where every tree operation must refuse with `NoMemory` and
  change nothing (spec `cell-hierarchy`, failures leave a consistent
  tree); and a node per entry costs more than 64 bytes.
- A slab with a sorted index of identifiers: removal still moves the
  index (8 bytes per cell instead of 64), O(n) all the same.
- Compacting at a fixed count: a burst of ends would pay one full pass
  at a fixed interval, whatever the tree's size.

## D2: A resume appends

`rekey` gives a resumed cell a fresh identifier, the largest so far in
the kernel. It moves the node to the new identifier's place (the end:
no node moves) and leaves a gone node in the old place. Within 16 nodes
of that place it rotates them, as before. The insertion may need to grow
the vector; if that fails, `rekey`, which reports no error, rotates too,
allocating nothing.

## D3: Identifiers that come back

The kernel never reuses an identifier, but the tree's interface allows
it (host tests do). Adding a cell, or rekeying to, an identifier that
has a gone node first drops every gone node (one pass), so identifiers
stay unique in the vector.

## D4: What sees gone nodes

Only the vector: `len`, `iter`, the roots (`children(HOST)`), `check` and
equality skip them, and trees compare by the cells they hold, so a
refused operation that dropped gone nodes on the way still leaves an
equal tree.
