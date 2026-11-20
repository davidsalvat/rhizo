# Design

## D1. The cell stays the unit of the run queues; idle CPUs join it

The run queues of `add-smp-scheduling` keep holding whole cells: fairness
between cells, step counts, and every host operation (suspend, fork,
freeze, kill) keep working on a cell that is either in the scheduler with
all its threads stopped, or out of it. What changes is what "out of it"
means: a cell taken out may now be run by several CPUs, one thread each.

`rhizo_sched::Gangs` records those cells (a pure structure, used under the
host's lock like the queues): for each, the CPUs that run it and what it
offers to other CPUs (runnable threads no CPU runs, and the earliest
deadline of its waiting threads). A CPU that finds no cell in any queue
(its own, then stealing) joins the first started cell that offers a
thread, taking one; a CPU leaves when its thread blocks and the cell has
nothing else for it, when its thread yields, and at the end of its slice,
always (so it looks at the queues again: a queued cell is preferred over
another thread of a running one). Exactly the last CPU to leave gets the
cell back and files it as before (runnable, waiting on descriptors,
asleep, parked) or frees it when it ended. Host property tests drive
random starts, joins, offers and leaves of several CPUs against a model.

Fairness: a cell gets more than one CPU only from CPUs that would
otherwise idle, so "thread count does not buy time" still holds whenever
cells wait. Threads of a running cell take turns by identifier (round
robin, D5), so none starves.

Lost: run queues of threads (a thread-level scheduler would make every
host operation stop threads one by one, and creating threads would buy
CPU time); gang scheduling that reserves CPUs for a cell's threads.

## D2. One CPU, one thread, or stepping: the single-CPU path, unchanged

A CPU running a cell alone keeps `&mut Live` and runs it exactly as
before (design D2 of `add-threads`): no lock, registers in the thread
table, lazy FP/SIMD switching. That path is taken whenever another CPU
could not join anyway: one CPU online (`smp=1`), `Host::step` (one CPU
stepping through cells), or a cell with one thread. A cell becomes shared
(a gang) when a CPU takes it from a queue with several threads on a
machine with several CPUs, or when its thread creates another in the
middle of such a slice (the CPU switches to the gang path right after the
`clone`, keeping the same thread). The `smp=1` metrics
(`cell.thread_create_us`, `cell.futex_wake_ns`, `cell.run_us`) therefore
run the same code as before.

Lost: one code path for both (the gang path costs a lock and register
copies per system call; the single-CPU benchmarks would pay for nothing).

## D3. A gang is the cell behind its own lock

Going shared, the CPU moves the `Box<Live>` into a `Gang`: the cell behind
a `sync::Global` (the per-cell lock), plus what CPUs read without it (the
cell's identifier, root and tag; the clean set of TLBs as an atomic; an
ending flag; a hold flag, D7). The gang is leaked to a raw pointer while
CPUs run it and registered in `Shared::gangs`; the last CPU to leave,
having removed it from `Gangs` under the host lock, owns it alone again
and turns it back into `Box<Live>` (one allocation per shared stint).

Under the cell lock: system calls, faults that change the cell's tables
(lazy resume, stack growth, copy-on-write), choosing a thread and putting
one back, the ending. Lock order: cell, then `SHARING`, then the host's
`Shared`, then `NET` (and the allocator locks inside). A CPU holding
`Shared` never takes a cell lock, so offering threads (D5) from inside a
cell's section is safe.

Lost: a reader-writer lock (most calls write the thread table anyway);
finer locks (files, memory map, threads) for calls that would not
conflict: every call already holds `NET` (design D9 of
`add-smp-scheduling`).

## D4. A running thread's registers stay with its CPU

The thread table keeps every thread, but a thread running in a gang has
its registers in its CPU's local copy (the table's copy is stale until the
thread is put back). For a system call the CPU makes the thread current
(`Threads::make_current`, a swap of two entries when another thread was
current) and copies its registers in, so `Abi::handle` and its tests are
unchanged; it copies them back out if the thread keeps running. A thread
that blocks, yields, is preempted or exits is put back under the lock.

FP/SIMD state moves eagerly when a thread is put back: if the CPU's
registers hold it, they are saved into the table before another CPU can
choose the thread. While it runs, the switch stays lazy (the first FP
instruction traps and loads it). Going shared, the CPU first saves the FP
state of whichever thread of the cell owns its registers.

Lost: the ABI taking the calling thread separately (no swap, but every
call site and test of `rhizo-abi` changes); boxing each thread so a CPU
can run it in place (an allocation per thread, and moves of the table
would still race with CPUs in user mode).

## D5. Choosing threads and offering them

`Threads::pick(now, after, busy)` wakes the threads whose deadline passed,
then returns the runnable thread with the smallest identifier above
`after` (wrapping), skipping those `busy` says run on another CPU; each
gang remembers the last identifier chosen. After every section under the
cell lock, the CPU computes what the cell offers (runnable threads no CPU
runs, earliest deadline of the others) and, if that changed, records it
in `Gangs` and wakes as many idle CPUs as there are threads offered. A
joining CPU takes one offered thread when it joins (the count goes down
in `Gangs` under the host lock) and chooses it under the cell lock; if
another CPU took it meanwhile, it records the true offer and leaves. So a
futex wake, a `clone` or a joiner's wake-up on thread exit reaches an idle
CPU through the existing wake-up interrupt, and an idle CPU waits no
longer than the earliest deadline of any running cell's threads.

Lost: waking a specific CPU per thread (no affinity yet); offering threads
that wait on descriptors (they still wake when the cell is filed).

## D6. Ending on every CPU

A thread that ends the cell (`exit_group`, the last `exit`, a fault)
records the ending under the cell lock, sets the ending flag and
interrupts the other CPUs running the cell's threads. Every CPU looks at
the flag before entering the cell's code and after any exit from it, and
leaves; the last frees the cell. Its threads may run a few instructions
more on the other CPUs, but no system call of theirs takes effect once
the flag is set (they look at it under the lock).

## D7. Shootdowns on x86_64: interrupt and hold

`INVLPG` is local; PCIDs keep translations across switches. Each CPU
publishes, in a per-CPU word, the cell whose code it runs and a sequence
number (odd while in user mode). A section under the cell lock that
shrinks the cell's translations (the TLB double of its system call or
fault sees a flush) first sets the gang's hold flag, makes the clean set
the current CPU only, then interrupts every other CPU whose word says it
runs this cell's code, and waits until each word changes (the CPU left
user mode). Entering user mode, a CPU publishes its word, then reads the
hold flag (both sequentially consistent: either it sees the hold, or the
holder sees it in user mode and interrupts it); while the hold is set it
withdraws and waits for the cell lock. The holder releases the hold at the
end of its section. So other CPUs are out of the cell's code from the
first unmapped page to the end of the call, frames are freed in place (no
deferred list), and they drop the PCID when they enter again (their clean
bit is clear). One interrupt round per call or fault, whatever the number
of pages. Waiting holds the cell lock, which is safe: a CPU leaves user
mode on the interrupt without taking any lock.

AArch64's `TLBI VAAE1IS` completes on every CPU before the frame is freed
(`DSB ISH`): the clean set stays full, no word is published and no
interrupt is sent. With other CPUs running the same tables, a
copy-on-write copy changes a valid entry's output address, which the
architecture allows only break-before-make: `resolve_write` (`rhizo-mm`)
now clears the entry and flushes it through the caller's TLB before it
points the entry at the copy (on x86_64 that flush is what holds the other
CPUs out). Another CPU that touches the page in between faults, waits for
the cell lock, finds the page mapped and runs on.

Lost: lazy shootdowns by generation numbers (would leave freed frames
reachable until the next switch); deferred frees with one interrupt at the
end (frames of a large unmap kept until then, a list per call).

## D8. Benchmarks

`multi_core` in `bench-cell` (4 vCPUs) adds `parsum` with 1 and 4 threads
(the cell measures its own computation with `clock_gettime`; median of 3
rounds each), reported as `cell.thread_scaling_pct = 100 * t4 * 4 / t1`
like `cell.scaling_pct`, and `shootdown bench`, whose first thread maps,
writes and unmaps one page 200 times while three other threads spin on
the other CPUs, reporting the median `munmap` (`cell.shootdown_us`). On
AArch64 the same figure shows the broadcast invalidation alone. The
metrics start ungated, like the other multi-core metrics.
