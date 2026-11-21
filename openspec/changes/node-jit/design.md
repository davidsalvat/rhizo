# Design

This change carries out design D6 of `add-node-workload` ("write-or-execute
pages, toggled on faults"), refined below where the code met it.

## D1. What V8 asks for

`strace` of the pinned Node.js (v24.22.3, V8 13.6) on x86_64 Linux, with
its JIT:

```text
mmap(hint, 536870912, PROT_NONE, MAP_PRIVATE|MAP_ANONYMOUS|MAP_NORESERVE) = B
madvise(B, 536870912, MADV_DONTFORK)
mprotect(B + 64 KiB, 536608768, PROT_READ|PROT_WRITE|PROT_EXEC)
madvise(B + 64 KiB, 536608768, MADV_DONTNEED)
...                                  (code written and run in place)
madvise(<code pages>, 262144, MADV_DONTNEED)
munmap(B, 536870912)
```

V8 never calls `mprotect` on its code again (the flag that made it flip
pages, `--write-protect-code-memory`, is gone; with protection keys, which
no AArch64 CPU has, it would flip `PKRU` instead). On AArch64 the range is
128 MiB (branches reach ±128 MiB). Two facts follow: the permission must be
given per page by the kernel, on faults; and a whole code range is asked
for at once, of which V8 writes a few MiB.

## D2. Write-or-execute pages

A page the cell asks writable and executable carries a software bit in its
leaf entry, `PageFlags::WRITE_OR_EXEC`: x86_64 bit 10, AArch64 bit 56
(both architectures leave those bits to software in leaf entries, blocks
included; the copy-on-write mark keeps x86_64 bit 9 and AArch64 bit 55).
The hardware bits say which of two states the page is in:

| State | Writable | Executable | Copy-on-write mark |
|---|---|---|---|
| writing | yes, or copy-on-write | no | when its frame may be shared |
| running | no | yes | never |

No operation writes an entry with both write and execute rights: `leaf`
encodes the flags it is given, and every caller asking for
`WRITE_OR_EXEC` passes the writing state (`WRITABLE | NO_EXECUTE`); the
two transitions below each remove one right before adding the other. A
property test drives random sequences of maps, protects, forks, writes,
fetches and clones over both formats and checks every leaf after each
step.

Alternatives: memory protection keys (the pages are W+X in the tables and
only a register keeps them from being written, not W^X; none on AArch64);
two mappings of the same frames (V8 does it only on Apple platforms);
honouring `mprotect` RW→RX only (V8 never asks).

## D3. The two transitions

**Writing → running, on an instruction fetch** (`resolve_exec` in
`rhizo-mm`). The tables on the way are made the cell's own (they may be
shared with a fork), then the entry becomes read-only and executable, its
copy-on-write mark cleared (a running page is read-only for its own
reason; a frame still shared is copied when the page is written next, see
below). The kernel then flushes the page's old translation from every CPU
that may hold it, through the same `CellTlb` as copy-on-write: a broadcast
`TLBI VAAE1IS` on AArch64; on x86_64 the gang's hold (the other CPUs
running the cell leave its code and flush on re-entry, design D7 of
`add-parallel-threads`) and, with KPTI, the cell marked stale on every CPU
(design D3 of `add-kpti`). On AArch64 it then cleans the data cache to the
point of unification and invalidates the instruction cache for the page
(`sync_code`, as for loaded images). Only then does the thread retry its
fetch: no CPU can still write the page through a stale translation while
its instructions are fetched.

**Running → writing, on a write.** `resolve_write`, the copy-on-write
resolution, learns that a write-or-execute page may always be written: a
frame still shared is copied (break before make, as before), and the
entry becomes writable and not executable. A write by a system call on
the cell's behalf takes the same path (the `Pager` already resolves
copy-on-write there), and so does the kernel's write fault handling, for
cell code. Nothing new is added on the path of ordinary copy-on-write
faults: the check is on the branch that used to refuse the write.

Fault kinds: `UserFault::exec_denied` is a page fault with `P` and `I/D`
set on x86_64 (an instruction fetch from a present page that forbids it),
an instruction abort from EL0 (EC `0x20`) with a permission fault (IFSC
`0b0011xx`) on AArch64. A fetch from a missing page is a translation fault,
`missing_page`, as before.

**A write from the page itself.** An instruction that writes the page it
runs from can never complete without the page being both writable and
executable: each transition would undo the other. The kernel reports such
a write fault as a fault and ends the cell, instead of letting it flip
forever; on Linux it would succeed. V8 does not do it (its code writes to
the heap, not to code space; the C++ runtime that writes code runs from
the executable's pages).

## D4. Pages from two threads

A page written by one thread while another runs it (V8's concurrent
Sparkplug batches, or a GC moving code next to running code) bounces: each
change is a fault, a table write, a flush and, on AArch64, cache
maintenance. Progress relies on the faulting thread retrying right after
its change, before the other thread's next fault is handled: that one must
take the fault, wait for the cell's lock (held by the first until its
change is done), and walk the tables, while the first only returns to its
instruction. The test cell `libc_jit` measures it: one thread runs a
function in a page while another writes a second function in the same
page, both a fixed number of times; both finish, and the kernel counts the
changes (`Count::WxFlips`). Measured on QEMU (design D10), the bounce
costs time but never stalls; no bound beyond that is needed. If it ever
were, V8's `--no-concurrent-sparkplug` avoids the pattern.

## D5. Write-or-execute memory backed on first touch

Every other reserved address (`PROT_NONE`) gets a zeroed frame when
`mprotect` makes it accessible (spec `linux-abi`, reserved mappings). For
V8's code range that is 512 MiB zeroed per spawn, a 1 GiB VM full, and
512 MiB in every snapshot. So memory asked writable and executable with
no frame behind it is not backed: an anonymous `mmap` with
`PROT_WRITE|PROT_EXEC`, or the reserved holes an `mprotect` with those
rights covers, become a *write-or-execute range*, and each of its pages
gets a zeroed 4 KiB frame, writing, the first time cell code or a system
call touches it. Pages already mapped when `mprotect` asks for both rights
keep their frame (and its size) and become writing.

The ranges (`rhizo_cell::space::WxRanges`) are sorted, merged where they
touch, and at most 64 per cell (spec `isolation`): an `mmap` or
`mprotect` that would need more fails with `ENOMEM`. `munmap`, `MAP_FIXED`,
`mprotect` with `PROT_NONE` and `mprotect` with anything else than both
rights remove the part of the ranges they cover first, then do what they
did before (an `mprotect` to read-only over an untouched part backs it
with zeroed pages, as for other reserved addresses). Every change to the
ranges is computed and checked first: a call that would need a 65th range
(a new one, or a split, as Linux's `munmap` can fail when a split needs one
mapping too many) fails with `ENOMEM` having changed nothing. An `mprotect`
with both rights checks every page before it changes one, and skips a hole
one empty table entry at a time (V8's 512 MiB at once), unless a lazily
resumed cell still has pages pending, which may sit in a hole.

They live where the cell's files live, in the kernel's record of a live
cell, and system calls reach them through `Context::wx` as they reach the
files: user-memory access (`Context::read`, `Context::write`, and the
piecewise copies of files, pipes and sockets) goes through a pager that
backs a page of a range it faults on (`Pager::back_wx`, the kernel's
allocator behind it) and retries. Alternatives: in `Cell` (system calls
get `&mut Cell` and the `Context` separately, so user-memory access, which
only has the `Context`, could not see them); markers in invalid page-table
entries (every walk that skips invalid entries, clone, snapshot, unmap,
`hole_end`, would have to learn them); backing every reserved address on
first touch (changes the rule glibc's guard pages rely on).

4 KiB pages only: a 2 MiB page would make every write of V8 flip 2 MiB of
running code.

## D6. Fork, templates and copy-on-write

A fork shares tables and frames as before. A running page is read-only,
so sharing it needs nothing; a writing page is writable, so the tables'
copy-on-write marks it like any writable page. Both resolve on a write by
`resolve_write`, which copies a frame still shared whatever the state. A
fetch from a writing page of a fork makes its tables its own first, so the
other cell's entry does not change. Templates' clones (`clone_lower_half`)
copy writing pages and share running ones, which a write copies later.
The ranges are copied with the cell (fork, template, spawn from a
template).

## D7. Snapshots: version 9

Permission bit 8 of a page record is write-or-execute. Such a record is
either writable and not executable (writing; a copy-on-write page records
writable, as before) or read-only and executable (running); anything else
is refused (`BadPage`). The header's reserved `u16` becomes the number of
write-or-execute ranges, at most 64, stored after the threads as
`(start, end)` pairs, page aligned, non-empty, sorted, not touching, below
`USER_END` (`BadRanges` otherwise). Running pages a cell shares are
referred to like other read-only pages; restored, they keep the bit and
resolve their next write by copy. A resumed cell gets the ranges back, and
its untouched pages there still come zeroed on first touch.

Older snapshots are refused, as every version change did. Version 8 is
`add-cell-quotas` (merged first: quotas in the state words); this change
takes 9 on top of it.

## D8. AArch64 caches from EL0

V8 on AArch64 reads `CTR_EL0` for the cache line sizes, then cleans the
data cache (`DC CVAU`) and invalidates the instruction cache (`IC IVAU`)
by address after writing code; GCC's `__clear_cache` does the same. With
`SCTLR_EL1.UCT` and `UCI` clear (QEMU's reset value), all three trap to
EL1 and the cell is killed (`system register trap`). Every CPU now sets
both in `user::init_cpu`, as Linux does. They change no state outside the
cell: cleaning or invalidating cache lines of a cell's own readable memory
only makes them agree with memory, which the kernel does itself on every
change to running. x86_64 needs nothing: its instruction cache follows
stores, and `CLFLUSH` is unprivileged.

## D9. Tests

Host: entry encodings of the bit on both formats; `resolve_exec` and
`resolve_write` on write-or-execute pages, alone, after a fork (each side
copies, the other's entry unchanged) and in clones; the property test of
D2; `WxRanges` (add, merge, remove in the middle, bound); `mmap`,
`mprotect`, `munmap` and user-memory access in `rhizo-abi`; snapshot round
trip of both states and the ranges, and refusal of bad records and ranges.

In the kernel, the C cell `libc_jit` (also run natively by the
differential check): it maps write-or-execute memory both ways (`mmap`,
and `mprotect` of `PROT_NONE` addresses, as V8), writes a function
returning a number, calls it, rewrites and calls it again; prints `ready`
and yields, where the tests fork or suspend it; then rewrites and calls
from the copies; then a thread writes code that another runs, in turns,
and finally the bounce of D4. It runs plain, forked (each copy rewrites
its own page), suspended and resumed eagerly and lazily, and on 4 CPUs
(`ktest-smp`, the two threads on two CPUs). Node.js runs without
`--jitless` in `node_answers_as_on_linux` and
`a_node_cell_suspends_resumes_and_forks`.

## D10. Metrics

`workload.node_jit_cold_us` and `workload.node_jit_warm_us` are measured
exactly as `workload.node_cold_us` and `workload.node_warm_us` (which keep
`--jitless`), without the flag, in the same `node` benchmark kernel
(`--workload node` measures all of them; its line `rhizo-node-jit`).
`workload.node_jit_flips` is the number of changes between writing and
running in one more cold run, counted after the timed ones (counting adds
to every system call), the cost in faults that W^X adds to the JIT.
Figures from CI are in the pull request.
