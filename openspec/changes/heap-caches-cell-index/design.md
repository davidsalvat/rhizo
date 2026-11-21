# Design

## D0. Profile: where the heap and the lookups cost

Measured on x86_64 QEMU `microvm`, under `-icount` (one nanosecond per
instruction) unless said otherwise, with temporary counters in the heap
and around the lookups (not committed).

| What | Measured | Share |
|---|---|---|
| Heap allocations per operation | spawn 2.1, fork from a template 1, `hello` run to its exit 4, `pipe_rtt` and `syscall` 0 | at about 40 instructions per allocation and free, 0.3 % of `cell.spawn_insns`, 0.6 % of `cell.fork_insns`, nothing of a system call |
| Heap lock on 4 CPUs | parallel breakdown in CI: 8 waits in 22 ms of CPU time (frames: 54); server workload on 4 vCPUs (TCG): 3,265 waits, 225,604 spins in 18 s of CPU time | well under 0.1 % of CPU time |
| Cell lookups at 10,000 cells | 900 lookups (kill, suspend, fork every 10 ms) took 26.4 M of the host operations' 77.2 M instructions: 29,000 each | a third of every host operation |
| Internal waste at 10,000 cells | at the end of the fill, 10,000 live blocks of 1345 to 1360 bytes (one kernel record per worker) served from the 2048-byte class; the other live blocks 16 to 192 bytes, 2,000 of each kind | 690 bytes per worker: 2.5 % of `density.cell_bytes` (27,940) |
| Internal waste in the laptop workload | 50 agents live: small blocks 231 KB requested (estimated from 16-byte buckets), 332 KB used | 100 KB, 1.5 % of `laptop.memory_bytes` |

So the heap's speed is not worth a change: a per-CPU cache (magazine)
would replace an uncontended lock with a per-CPU index and a bookkeeping
of its own, about as many instructions, and could only win the waits it
removes; and its blocks would have to be flushed from every CPU for
`trim`, the statistics and the leak tests to stay exact. Its waste and the
lookups are.

## D1. Classes between the powers of two

| Option | Waste of a 1350-byte record | Cost |
|---|---|---|
| Powers of two (before) | 698 bytes, two per page | — |
| **Fractions of a page (chosen)**: the largest multiple of 16 of which a page holds 85, 42, 21, 12, 9, 6, 5 or 3 blocks (48, 96, 192, 336, 448, 672, 816, 1360) | 10 bytes, three per page | a 129-byte table from size to class |
| Half steps (48, 96, 192, 384, 768, 1536) | 186 bytes, but two per page: a 1536-byte class saves nothing per page | — |
| Classes tuned to today's record sizes | least | breaks with the next field added |

A class's blocks are only 16-byte aligned unless its size is a power of
two, so a request aligned to more than 16 bytes keeps the old path:
rounded up to the power of two of its size and alignment. The power-of-two
classes keep indices 0 to 7 and the new ones take 8 to 15, so `plan`'s
mapping of every power-of-two request is unchanged. `trim` works by page
as before: `PAGE_SIZE / size` blocks per page, the same test for a whole
free page. The size-to-class table replaces the power-of-two rounding on
the allocation path: a compare, a shift and a load.

Kernel records of 1345 to 1360 bytes now take three to a page instead of
two: 6.9 MB less at 10,000 cells. If the record grows past 1360 bytes it
falls back to 2048; the waste metric of D0 would show it, and the class
set needs no change for the other requests.

## D2. Sleepers by hint

The server workload's workers spend most of their time asleep (6,000 of
the 10,000 in the sleep list) or waiting on a socket (in the waiting set,
already a map by key). A lookup by key scanned the run queues, then the
waiting set, then the sleep list: 24-byte entries, 29,000 instructions at
10,000 cells.

| Option | Lookup | Cost per switch | Cost per sleep |
|---|---|---|---|
| Keys next to entries (before) | O(n) | none | none |
| A map from key to place, kept exact | O(log n) | a map update on every take and give-back (the switch is 896 instructions, gated) | a map update |
| A slot table by key with (queue, arrival number) | O(log n) in the queue | a store on every give-back | a store |
| **Hints for sleepers only (chosen)** | O(log n) for a sleeper whose hint holds; O(n) otherwise | none | a store |

The sleep list is sorted by wake time, so a sleeper's wake time finds it
with a binary search, and only `sleep` gives an entry a wake time. The
scheduler keeps a table of wake times indexed by the key's low bits
(a power of two, at least as many slots as sleepers, grown and refilled
from the sleep list when the sleepers outnumber it). `sleep` writes the
slot; nothing clears it. A lookup reads the slot of its key, binary
searches the sleep list for that time, and looks for its key among the
entries waking then. The hint can be stale (the sleeper woke, was
removed, or a sleeper with the same low bits took the slot), so a miss
falls back to the old search: the result is always the search's.

Cell identifiers are handed out in order, so live cells' identifiers are
close and seldom share a slot. Entries parked for good (no deadline) all
wake at `u64::MAX`; a lookup among them compares keys over that run only.
The lookups run in the order hint, waiting set, run queues, sleep list:
keys are unique among entries, so any order finds the same entry.

Runnable cells are still found by comparing keys over the run queues,
which hold the cells ready to run: a few, unless cells are CPU bound.
Indexing them means a store on every give-back, the switch path, which
this change must leave as it is.

## D3. Host operations as metrics

The server workload already timed suspends, resumes and forks during its
load and printed their medians in its breakdown line. It now times kills
too, and `xtask` reports `density.suspend_us`, `density.fork_us` and
`density.kill_us` from that line (where present, so older lines still
parse). They have no baseline yet: the first CI run gives one.

## D4. Results

Server workload, x86_64 QEMU `microvm`, 1 vCPU, under `-icount` (so the
figures are instructions), before and after:

| Metric | Before | After |
|---|---|---|
| `density.fork_us` | 58.6 | 34.2 (-42 %) |
| `density.kill_us` | 38.2 | 10.9 (-72 %) |
| `density.suspend_us` | 158.9 | 142.2 (-11 %) |
| Host operations in all (`host_ns`) | 76.7 M | 61.2 M (-20 %) |
| `density.cell_bytes` | 27,936 | 27,168 (-2.7 %) |
| `density.oom_cells` | 46,831 | 48,246 (+3.0 %) |
| Small-block waste at the end of the fill (estimated from 16-byte buckets) | 7.2 MB, 34 % of the small blocks | 0.14 MB, 1 % |

`cell.switch_insns`: the code of the switch path (`give_back`,
`requeue`, `take`, `next`) is instruction for instruction the same; only
field offsets of `Scheduler` moved (its new `hints`). The measured figure
moved by one instruction (x86_64 896 to 895, AArch64 838 to 839 on QEMU 8
here), from the window it is measured over, which includes starting the
run and ending both cells. The forks (`cell.fork_insns` and the others
that fork) gained 31 instructions on x86_64 (+0.46 %), from inlining
choices the new allocator code changed around `child_of` and
`spawn_from_template` (the allocator's own fast path is four instructions
shorter); on AArch64 they lost 50 to 64 (-0.6 to -0.9 %).
