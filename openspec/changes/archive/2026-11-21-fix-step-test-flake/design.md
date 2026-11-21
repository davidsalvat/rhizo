# Design

## D1. The kernel is right; the cell defines its steps

A thread whose sleep is over when it blocks is runnable, and the kernel
runs a cell's next runnable thread at once, in the same slice (spec
`scheduling`, "Threads share their cell's time"): `run_slice` checks
`threads.next(now)` after a call blocks. For a cell with one thread that
is the same thread. Ending the slice there instead would cost a scheduling
round for nothing and is not what the spec says, so the kernel stays as it
is.

The test suspends after every scheduling step and asks for 40 suspensions,
one per step of `counter`'s work. Options:

| Option | Deterministic | Checks as much |
|---|---|---|
| **`counter` yields after every step, after its sleep where it sleeps (chosen)** | yes: a yield always ends the slice | yes: same assertion, same output, and a sleep still in progress adds a suspension of a sleeping cell, as before |
| Count suspensions minus steps the cell ended asleep (at least 36) | yes | less: a sleep that never blocked would pass |
| Sleep longer than any stall | no: a stall has no bound, and the cell's output is also checked on Linux, so long sleeps slow every run | yes |

`counter`'s description already said it yields after every step. The other
users of `counter` (`cells_in_parallel_stay_isolated`,
`suspend_between_parallel_runs_is_invisible`, the Linux differential check)
compare its output only, which does not change.

## D2. Fill the snapshot in place

`Snapshot::capture` grew its buffer with `Vec::resize(len, 0)` before
copying the pages into it. Unoptimized, `resize` writes one byte per
iteration: about 160 ms for 4 MiB under QEMU, more than the rest of a
suspend. An optimization level for `rhizo-snapshot` alone does not help
(measured: the generic code is shared with unoptimized crates). `vec![0; n]`
asks the allocator for zeroed memory, which zeroes with `memset` in every
build; the buffer is sized once for everything (header, records, pages,
files, sockets) and filled at known offsets. Release builds already zeroed
with `memset`: the instruction count of `cell.suspend` stays within 0.02 %.
