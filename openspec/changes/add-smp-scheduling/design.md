# Design

## D1. A cell is the unit a CPU runs

A CPU takes a whole cell out of the scheduler, runs its threads for one
slice exactly as before (design D2 of `add-threads`), and gives it back.
While it runs, the cell belongs to that CPU alone, so the system-call
handler, the thread table, the files and the pager keep their `&mut`
access and need no lock. A cell never runs on two CPUs at once.

Lost, for now: threads of one cell on several CPUs. They need a lock per
cell around system calls, a running thread per CPU in the thread table,
futex wake-ups across CPUs, and TLB shootdowns while the other threads run
(x86_64 IPIs). That is the next change; cells already get every core.

## D2. Per-CPU queues in `rhizo-sched`, with arrival numbers

`Scheduler` keeps one `VecDeque` per CPU and stamps each entry with an
arrival number. Taking: own queue first (cache and TLB stay warm), else the
entry with the lowest arrival number among the other queues' fronts (the
one that has waited longest). Giving back: own queue, unless it is already
longer than the shortest; then the shortest (queues stay within one entry,
without a periodic balancer). New entries go to the shortest queue.
Sleepers wake onto the queue of the CPU that finds them. `next_oldest`
takes the lowest arrival number of all, which with every CPU stepping
through it is exactly the single round robin of before, so `Host::step`
keeps its meaning (and step-counting tests their counts) on any number of
CPUs. Property tests drive several simulated CPUs through random
interleavings (the "loom-style" check, without threads: the structure is
used under a lock, so interleavings of whole operations are what matter).

Lost: stealing from the longest queue (an entry at its back can wait
behind newer ones) and balancing on a timer (needs a tick per CPU when
idle).

## D3. One lock per host, held to take or give back a cell

`Host` keeps its scheduler, tag pool and the cells that ended in a
`Shared` behind one `sync::Global` (the spinlock of `add-smp-boot`). A CPU
holds it to take a cell (and, finding none, to mark itself idle) and to
give one back; never while a cell runs, never while a cell's memory is
freed. `Host::run` posts `run_cpu` (with the address of `Shared`) to every
secondary CPU through its mailbox, runs cells on the boot CPU too, checks
the caller's stop condition there, then sets `stopping`, wakes everyone and
joins them. Between runs the boot CPU has `&mut Host`, so it reaches
`Shared` without the lock (`get_mut`).

Ended cells go to `Shared::ended`, which the boot CPU moves to
`Host::finished` (so `finished(&self)` still hands out references). Idle
bits are set under the lock by the CPU that finds nothing and read under
the lock by the CPU that gives a cell back: either the taker saw the cell
or the giver saw the idle bit and sends a wake-up, so none is lost. The
boot CPU does not wait while ended cells are unseen (`Take::Look`).

Lost: a lock per queue (more locks to order, for a section of a few
hundred nanoseconds per slice; the multi-core benchmark shows whether it
matters) and lock-free queues.

## D4. A CPU lets go of a cell when it gives it back

With several CPUs online, a CPU giving back a cell first saves the FP/SIMD
registers if one of the cell's threads owns them (eager instead of lazy:
the next CPU to run the cell reads them from memory) and switches its own
lower-half root to the kernel's or the empty table (`user::leave_user`,
without a flush). Otherwise another CPU could free the cell's tables while
this one still had them loaded (x86_64 would keep translating the kernel
half through a freed root), or run a thread whose FP state sat in another
CPU's registers. One CPU online skips both, as before. Per-CPU state that
was global (loaded root, FP trap bit, FS base, FP owner) is indexed by CPU.

## D5. TLB coherence: broadcast on AArch64, a clean set on x86_64

AArch64 cell invalidations are inner-shareable (`TLBI VAAE1IS`,
`ASIDE1IS`): every CPU drops the entry at once. x86_64 `INVLPG` and CR3
loads act on one CPU, and with PCIDs a CPU keeps a cell's translations
across switches. Since a cell runs on one CPU at a time (D1), no shootdown
interrupt is needed: each cell carries `clean`, the CPUs whose TLB holds
nothing stale for it. A new cell (or a cell with a new tag) has none;
entering on a CPU outside the set loads CR3 without the no-flush bit (the
PCID is dropped on that CPU) and adds it; when the cell's translations
shrink (a system call that flushed, a copy-on-write fault, a fork) the set
becomes the current CPU only. This subsumes the "fresh tag" flush of
`add-scheduler`: one CPU, one bit, same flushes as before. On AArch64 the
set is always full after the first entry (`TLB_FLUSH_IS_BROADCAST`).

Lost: IPI shootdowns (needed only once threads of a cell run on several
CPUs at once) and a per-CPU table of (tag, generation) (16 CPUs × 4096
tags of state for what one word per cell expresses).

## D6. Copy-on-write sharing changes one CPU at a time

Forked cells share page tables and pages with reference counts
(`add-fork`). Resolving a write through a shared table copies the table,
takes a reference on everything below it, then drops one on the table;
freeing a hierarchy drops references and frees what is no longer shared.
Each step is atomic, but two cells sharing a table doing this on two CPUs
interleave the steps: a table could be freed without dropping its
children's references (a leak, found by `ktest-smp`), or copied while the
other changes it. A global lock (`SHARING`) is held around the system
calls, copy-on-write faults and teardown of cells that may share memory
(`Live::shares`: forked, a fork, or from a template). Cells spawned from
an executable or resumed share nothing and never take it.

Lost: a lock per sharing group (cells forked from one ancestor), which
needs the group as state that survives forks and templates; and making the
copy-on-write walk lock-free.

## D7. Large heap allocations without contiguous frames

Cells running in parallel interleave their frame allocations, and memory
ends up fragmented in ways one CPU never produced: on x86_64 with four
vCPUs a 2.4 MB snapshot found no run of free frames with 100 MiB free. The
kernel heap's large allocations still take a contiguous run first (the
direct map, no mapping work); when none is left they take single frames
and map them one after the other in a window of the kernel half
(`heap::window`, one top-level entry, 512 GiB), whose tables exist before
any cell (x86_64 cell roots copy the kernel's top-level entries). Window
addresses are never reused, so a stale translation on another CPU can
never reach memory handed out later, and freeing needs no shootdown.

Moving cells between queues and CPUs moves `Box<Live>` (`add-smp-boot`
D8), so a slice moves a pointer, not 1.2 KiB.

Lost: reusing window addresses (needs kernel TLB shootdowns), and mapping
every large allocation (slower than the direct map for the common case).

## D8. Multi-core benchmark in the same `bench-cell` image

`bench-cell` measures the multi-core figures when it boots with several
CPUs (and only them; every other metric is measured with one CPU, as
before): sixteen `spin 5` cells (four per CPU) run to their end stepping on
the boot CPU alone, then on every CPU (`cell.parallel_us` per cell,
`cell.scaling_pct`); and 105 round trips to an idle CPU (post, wake-up
interrupt, run nothing, see it done; `cell.ipi_wake_us`). `xtask` boots it
with 4 vCPUs on every VMM after the one-CPU runs. The metrics start
ungated: there is no baseline from CI yet, and runners share their cores.

## D9. Sockets and the network card with several CPUs

`add-sockets-and-poll` and `add-virtio-net` landed while this change was
open. Their state stays where they put it: one `Net` behind a global lock,
taken around every system call (with `SHARING` first, for cells that
share memory). The waiting set moves into the per-CPU scheduler: any CPU
taking a cell first wakes the waiting cells onto its own queue when the
socket change counter moved (`Shared::net_seen`). A cell records the
counter its last call saw, under the same `Net` lock as its readiness
checks; filed to wait after a later change, it runs again at once
(otherwise another CPU could wake the waiters between the check and the
filing, and the cell would wait for a change that already happened). The
boot CPU alone polls the network card, as before; with several CPUs an
idle CPU looks again at least once per slice, a safety net behind the
wake-up interrupts.

Lost, for now: system calls of different cells in parallel. Every call
holds the `Net` lock (the ABI takes `&mut Net`), so calls serialize while
cell code runs in parallel; splitting it (sockets per cell, or a lock
taken only by socket calls) is the next measured step.
