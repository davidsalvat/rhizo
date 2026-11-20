# Design

## D1. A thread table inside the cell, as plain data

`rhizo_cell::threads::Threads` holds a cell's threads: the one that runs
(or ran last) inline, the others in a `Vec`, and the next identifier. A
`Thread` is its identifier, its `Registers` (FP/SIMD and thread pointer
included), its clear-tid address and a `Wait`: runnable, sleeping until a
time, or waiting on a futex (address, bitset, optional deadline). `Cell`
carries the table where it carried one `Registers`; everything else about
the cell (root, tag, program break, mappings, files) stays shared.

A single-threaded cell keeps its only thread inline, so spawning and
forking such a cell allocate nothing more than before. Switching threads
swaps two records (under 1 KiB each).

Lost: making each thread a scheduler entry with the shared parts behind an
identifier. That needs a second table for the shared state, lookups on
every system call, and per-thread fairness (D2). When cells get several
CPUs, the table stays: `Wait` and the futex logic do not care which CPU a
thread runs on; only "the running thread" becomes one per CPU.

## D2. The cell is the unit of CPU sharing

The run queue and sleep list keep holding cells. When a cell runs,
`Threads::next(now)` wakes its threads whose time has come and picks the
next runnable one after the current (round robin inside the cell); a
preempted or yielding thread puts the whole cell back in the queue. When a
thread blocks (futex, sleep, exit), the cell's next runnable thread runs at
once in the same slice: same root, same tag, no TLB work, no trip through
the run queue. When no thread can run, the cell sleeps until
`Threads::wake_at()`, the earliest deadline; with none it is parked at
`u64::MAX` (deadlocked, as it would be on Linux, but it can still be
killed, suspended or forked), and `Host::run` stops when only parked cells
are left instead of waiting forever.

Lost: per-thread fair share as on Linux. For agents sharing a machine, a
cell must not get more CPU by creating threads; and futex hand-offs inside
a cell stay as cheap as a function of the cell.

## D3. Futexes: the waits are in the thread table

A futex wait reads the word through the cell's address space (so lazy
resume and copy-on-write work as for any call), compares, and records
`Wait::Futex` on the calling thread; the result register is set when the
thread is woken (0) or times out (`-ETIMEDOUT`). A wake scans the cell's
other threads for waiters on that address whose bitset intersects, in
round-robin order, and makes up to `n` runnable. Private and shared futexes
are the same: a cell is one address space and no memory is shared between
cells. Waiters are keyed by virtual address.

Lost: a hash table of wait queues. Cells have few threads, a scan of a
handful of records is cheaper than hashing, and there is no second
structure to keep consistent or to serialize: the waits are in the
snapshot because the threads are.

## D4. Identifiers: per cell and stable

The first thread's identifier is the cell's identifier at spawn; new
threads get the next unused number of the cell. Identifiers travel with
the threads into snapshots and forks, because glibc keeps them in memory
(`pthread_t`'s `tid`, written by `CLONE_PARENT_SETTID` and
`set_tid_address`) and compares them (mutex owners). `getpid` keeps
returning the cell's current identifier, which changes with resume and
fork (spec `fork`: values derived from the identifier differ).

Lost: one global identifier space (every resume or fork would renumber
threads that memory still names by their old numbers).

## D5. `clone`: threads only

`clone` and `clone3` create a thread when `CLONE_VM | CLONE_SIGHAND |
CLONE_THREAD` are set (glibc's `pthread_create` passes them with
`CLONE_FS | CLONE_FILES | CLONE_SYSVSEM | CLONE_SETTLS |
CLONE_PARENT_SETTID | CLONE_CHILD_CLEARTID`); without them the call would
create a process, which cells do not have (the host forks cells), so it
returns `ENOSYS`, the answer libc expects from a kernel without a feature.
The child is the caller's registers with result 0, the new stack pointer
and thread pointer. Its FP/SIMD state is the caller's: the kernel switches
FP state lazily, so the caller's may be in the CPU only; the ABI asks
`Services::save_fp` to bring it up to date before copying. A new thread
costs one `Thread` record of kernel heap; `Services::can_store` is asked
first, so a cell creating threads without end gets `EAGAIN`, never the
kernel's reserve (spec `cells`, memory pressure).

## D6. Reserved mappings without a mapping list

`mmap(PROT_NONE)` already only moves the mapping cursor. `mprotect` with an
accessible protection now maps zeroed pages, with that protection, in every
hole of its range that lies below the mapping cursor (2 MiB pages where the
range allows), then changes the protection of the whole range. This is
what glibc needs for thread stacks (reserve, then open all but the guard
page) and per-thread arenas (reserve 128 MiB, open as the heap grows), with
no new per-cell state to snapshot.

Lost: a list of mappings (VMAs). It would also refuse `mprotect` on a hole
left by `munmap`, which Linux does with `ENOMEM`; Rhizo backs it instead. No
program relies on that error, and the list would be state in every
snapshot and fork. Pages are backed when opened, not on first touch: a
default 8 MiB glibc stack costs 8 MiB at creation (demand-zero memory is
out of scope).

## D7. Snapshot format version 4

```text
header   "RHZSNAP1", version u32 (4), machine u16, reserved u16,
         pages u32, threads u32                            (24 bytes)
state    brk_start, brk, mmap_next, next thread identifier  (4 x u64)
threads  per thread, the current one first: tid, clear_tid, wait kind
         (0 runnable, 1 sleep, 2 futex), futex address, futex bitset,
         time left in ns (u64::MAX: no deadline), pc, sp, gpr[31], tls,
         flags, fp[66]                          (107 x u64, 856 bytes each)
records, data, files: as in version 3
```

Deadlines are stored as time left, as the single sleep was, and become
deadlines again at resume. Decoding checks the thread count (at least one),
wait kinds, futex addresses (aligned, user half) and bitsets, and that
identifiers are unique, positive and below the next identifier. Version 3
snapshots are rejected: snapshots are not kept across kernel builds yet,
and a converter would be code nobody uses.

## D8. Forks copy every thread; templates expire time

A fork copies the thread table with the rest of the cell (D1), waits
included: a child of a cell whose threads wait on a condition variable
has the same waiting threads, woken by its own `pthread_cond_signal`.
`spawn_from_template` additionally ends the template threads' sleeps and
times out their timed futex waits (`Threads::expire_timers`): a template
is instantiated at an arbitrary later time, so its deadlines mean nothing,
and an agent request served from a template must start running, not wait
out a sleep that was in progress when the template froze (as before this
change, when the template's single sleep was dropped).

## D9. x86_64: `SYSRETQ` only for a thread that entered through `SYSCALL`

The x86_64 entry returns with `SYSRETQ` (faster) when the cell last left
through `SYSCALL`, because then RCX and R11 hold nothing it needs. That was
tracked per CPU and reset on a CR3 change, which no longer suffices: two
threads of a cell share CR3, and a thread preempted by the timer must get
its RCX and R11 back. The condition becomes a property of the registers
being entered: `SYSRETQ` when RCX equals the program counter and R11 the
flags it would restore, which is exactly when it restores the same state as
`IRETQ` (and always the case after a system call). The `threads` cell
checks RCX and R11 across preemptions while another thread makes system
calls.

## D10. Thread benchmarks measured by the cell

The `threads` cell has a `bench` mode that measures, with
`clock_gettime(CLOCK_MONOTONIC)`, creating and joining a thread (raw
`clone` plus a futex wait on the clear-tid word) and a futex ping-pong
between two threads (each hand-off: wake, block, the other runs), and
prints robust figures (minimum over five rounds of each round's median).
The `bench-cell` kernel runs it and prints `rhizo-threads create_ns=...
futex_wake_ns=...`; `xtask` turns them into `cell.thread_create_us` and
`cell.futex_wake_ns`. Measuring inside the cell excludes spawn and exit,
which other metrics cover.
