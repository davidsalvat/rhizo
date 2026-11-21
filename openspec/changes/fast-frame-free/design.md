# Design

## D1. The profile

All figures are from QEMU `microvm` under `-icount shift=0,sleep=off`, in
retired instructions, on x86_64, before this change.

`cargo xtask profile --accel icount` (cost accounting, with timing
counters added around the frame calls for this measurement only):

| Workload | Part | Instructions | Share of kernel time |
|---|---|---|---|
| `cell_loop` (101 `hello` runs) | `frames` (allocator and reference counts) | 667,739 | 10.0 % |
| | `copy` (copies and zeroing) | 1,483,267 | 22.2 % |
| | zeroing alone: 2,329 pages at about 542 each | 1,262,318 | 18.9 % |
| | `alloc_for_cell(1)`: 2,329 calls at about 72 each | 167,688 | 2.5 % |
| | bitmap `free`: 2,323 calls at about 91 each | 211,393 | 3.2 % |
| `fork_touch` (5 forks writing 50 MiB) | `frames` | 817,628 | 2.3 % |
| | bitmap `free`: 160 calls at about 4,613 each (large pages) | 738,185 | 2.1 % |
| | `alloc_for_cell` (large pages): 160 calls at about 80 each | 12,808 | 0.04 % |

The timing counters themselves cost about 10 to 15 instructions per call.

Four-CPU server workload (`cargo xtask bench --workload server --cpus 4`,
QEMU TCG, 10,000 cells), with each lock's waits printed at the end of the
load:

| Lock | Waits |
|---|---|
| `frames` (allocator) | 127 |
| `refs` (frame reference counts) | 109 |
| `heap` | 1,982 |

That run made 12,042 copy-on-write faults over 4.6 s of load. The frame
allocator is not where the CPUs wait.

What each design would save:

- **Per-CPU magazines.** A pop or a push still takes a per-CPU lock, or a
  per-CPU access with interrupts masked, and the free count used by the
  memory reserve must stay global. That saves about 30 to 40 instructions
  per alloc and free pair, or about 1 % of `cell.run_insns` and
  `cell.spawn_insns`, with no contention to remove. The cost: frames
  held per CPU are invisible to `alloc_large`, `alloc_contiguous` and the
  heap's `alloc_high` until drained, and the balance (`free_bytes`, the
  reserve check, the leak tests) must add the caches.
- **Idle-zeroed pool.** Zeroing is the biggest frame-related cost (about
  19 % of `cell_loop`'s kernel time). Every gated figure runs back to
  back, though, so the pool would be empty in all of them. The paths that
  zero (`PageTable::map`'s intermediate tables, anonymous mappings, stack
  growth, page-cache holes) would also need a zeroed-frame allocation
  through `rhizo-mm`, `rhizo-cell` and `rhizo-abi`. Zeroing speed is
  agent B's assembly primitives in the same wave.

## D2. Freeing a run a word at a time

`free_contiguous` used to check for a double free with
`(first..first + count).find(is_free)`: one bit test per frame, 512 for
a 2 MiB page. It now walks the run's bitmap words. `words(first, last)`
yields each word index with the mask of the run's bits in it, and the
first word where `word & mask != 0` gives the lowest frame already free
(`trailing_zeros`). That is the frame the panic message named before.
`update_range` walks the same words, so both share the helper.

For one frame (most frees) the iterator costs more than it saves. Its
first version made `cell_loop`'s `frames` part 5.9 % more expensive. So
`count == 1` tests and sets the single bit directly.

## D3. Verification

- Host tests: a double free inside a run spanning three words panics
  naming the lowest free frame; a run across words frees every frame
  once. The property test `random_sequences_keep_invariants` covers
  random runs.
- Instruction counts on both architectures (`--icount-only`), compared
  before and after in the pull request.
