# Design

## D1. Wait queues live in the sockets, filled by the calls that looked

Each socket in `rhizo_net::Net` keeps a wait queue: the keys (cell
identifiers) of the waiters that looked at it before they blocked. The
public calls that can report "not yet" (`poll`, `recv`, `send`, `accept`,
`connect`) note the socket they were given in a list of the current call.
At the end of every system call the kernel calls `Net::end_call`: if the
calling thread now waits on descriptors, its cell's key joins the queue of
each socket noted (once per queue); the list is cleared either way. A
`poll`, `select` or `epoll_wait` over several sockets joins all their
queues, since `epoll` reaches its sockets' readiness through `Net::poll`.

A change names the socket a waiter of which may now proceed, and moves
that socket's queue to `Net::woken`:

| Change | Socket woken |
|---|---|
| bytes queued (`send`) | the receiving end |
| a datagram queued | the receiving socket |
| bytes read from a queue that was full | the writer's end (the peer) |
| a connection queued | the listener |
| `shutdown` | the socket and, for writing, its peer |
| close | the socket's own queue and its peer (end of stream or reset) |
| a reset (connections never accepted, `settle` of a failed connection) | that socket |
| a frame or timer changed `smoltcp`'s state | every socket on the wire |

Reading from a queue that was not full wakes nobody: a writer waits only
on a full queue (`Net::send` returns `Again` only then), so the old
unconditional "room for a writer" change only woke readers for nothing.

The queues are kernel state only: snapshots and forks never see them. A
resumed or forked cell, or one the host woke, makes its waiting calls
again before it waits (D2), which fills the queues anew; this is what made
design D3 of `add-sockets-and-poll` decide against "pointers between
objects and cells that snapshots cannot hold": there are none to keep.

A key may stay in a queue after its cell stopped waiting there (it timed
out, or a change of another socket woke it). Its next wake-up is then
spurious but harmless (the cell looks again). When a cell ends or is
suspended, the kernel takes its key off the queues of the sockets it held
(`Net::forget`) before it releases them.

Lost: precise interest masks (a waiter for reading woken by a change that
only made room for writers); with the full-queue rule above, the workloads
show no such wake-up. Exclusive waits for `accept` (wake one): a woken
waiter that does not accept (killed, suspended, timed out) would strand the
connection, and the serving workers of the server workload wait in
`accept` only at their start.

## D2. The kernel wakes by key, under the host's lock; nothing is lost

The scheduler's waiting set becomes a map from key to entry (plus an
ordered set of deadlines), so `Scheduler::wake(cpu, key, f)` moves one
waiter to a run queue without looking at the others, and the next deadline
and the due waiters are found without a scan. The wake-all
(`wake_waiting`) is gone.

The CPUs drain `Net::woken` under the host's lock whenever they take a
cell or give one back (`wake_woken`): a key in the waiting set wakes that
cell (its threads waiting on descriptors become runnable and make their
calls again); any other key goes to `Shared::nudged`. A cell whose key is
nudged has its waiting threads woken the next time a CPU takes it from a
queue or gives it back (`nudge`).

No wake-up is lost between "nothing ready" and "filed as waiting": the
call joins the queues under the sockets lock in the same critical section
in which it found nothing ready, so a later change finds the key. If the
change comes before the cell is filed, the key is drained either by the
CPU giving the cell back (which drains before it files) or by another
CPU; in both cases under the host's lock, before or after the filing:
before, the key is nudged and the filing wakes the cell's threads; after,
the key is in the waiting set and wakes the cell. This replaces the global
change counter each cell recorded (`io_seen`, added by
`add-smp-scheduling` for the same race): per cell and per socket, it no
longer wakes a cell because an unrelated socket changed.

Between runs every live cell is in the scheduler, so nudged keys of cells
that no longer exist (a change drained after the cell ended) are dropped
at the end of `Host::run`.

A cell new to the scheduler (forked, resumed, a template's child, woken
by the host) is on no queue: when it is filed, its threads waiting on
descriptors are made runnable and make their calls again (before this
change the global counter, never seen by a new cell, did the same).

Lost: waking from the system call itself (it holds the sockets lock, not
the host's; taking the host's lock in every call that changes a socket
would serialize the CPUs on it); keeping a list of the cells in the
waiting set per socket in the scheduler (two structures to keep in step).

## D3. A cell's own pipes, event counters and `epoll` instances

Pipes, event counters and `epoll` instances live in a cell's file tables
(`rhizo-fs`), never shared with another cell (a fork copies them). Only
the cell's own threads change them, so the ABI wakes the cell's other
threads waiting on descriptors when a system call changed one: bytes in or
out of a pipe, an end closed, a counter written or read, an interest
added, changed or removed. `rhizo_fs::Tables::changes` counts these, and
`Abi::handle` compares it before and after the call. A thread that blocks
in that same call blocks after the others were woken, so it does not wake
itself.

Before this change such a thread woke only when some socket of some cell
changed (the global counter), or at its deadline.

Lost: wait queues per pipe (the threads of one cell are few; they look
again at no cost to other cells).

## D4. Measuring it

`prof` counts `IoWoken` (cells woken from the waiting set by a socket
change) and `IoWasted` (of those, the ones whose next system call waited
on descriptors again). The server workload prints both on its result line
and `xtask` reports `density.wasted_wakeups_pct`, lower is better. It is
listed under `ungated.ci`: its value is near 0, where a relative threshold
means nothing; the in-kernel test `only_the_cells_waiting_on_a_socket_wake`
requires none on a ping-pong between two cells next to an idle server.

The latencies count only cells that joined a queue, or whose event came,
after counting started (`prof::start`). The server's 10,000 workers are
made, runnable, before its load starts, and all run in its first tenth
of a second: before this change, their waits were under 1 % of the
1.8 million samples, but with 200,000 samples they were 5 % of them, and
`density.sched_p99_us` read 88 ms, the time it takes to make the cells, not
a scheduling delay of the load.

The system-call latencies change for the same reason. The 1.6 million
re-checks that found nothing were the cheapest calls (a `recv` that
returns `EAGAIN`). Without them the mix holds mostly calls that do work:
the total time in system calls halves (404 ms to 191 ms in CI), but the
percentiles of what is left go up (`density.syscall_p99_ns` from 0.8 to
1.5 µs on QEMU, 1.2 to 3.0 µs on Firecracker). Their baseline is updated
from this change's runs, with this justification. Changes allocate
nothing in the system call: woken keys are drained, and the list keeps
its room (`Net::drain_woken`).

`density.sched_p99_us` and `density.wake_p99_us` were ungated because
every socket change woke every waiting cell, which made their tails move
up to 5x between runners. Once two CI runs of this change agree, they are
gated, with the values of those runs in the `ci` baseline.

## D5. A wake from the host is not lost

`Host::wake` (a user's message, in the laptop workload) ended the sleeps of
the cell's threads. A cell still on its way to its sleep (preempted by a
tick between printing `ready` or `turn` and calling `nanosleep`) had none
to end: the wake was lost, and the cell slept for its hour. This is how
`a_woken_cell_runs_at_once` hung, in about one run in four of `ktest-smp`
under host load. It ended with `ready` as the last line. It was not a
missed wake-up between CPUs.

Now, when none of the cell's threads waits with a deadline, the wake is
kept on the live cell (`host_wake`), and the first system call after
which one of its threads waits with a deadline ends those waits at once.
The regression test `a_wake_before_the_sleep_is_kept` does not depend on
timing: `sleeper 3600000 yield` prints `ready`, yields, then sleeps for an
hour. The host wakes it at the yield, and two steps must see it end. Before
the fix, the test hangs exactly as the old one did.

The kept wake is host state: it is not part of a snapshot or a fork, like
the wake itself.

Lost: telling the caller the wake found nothing, so that it retries.
Every caller would need a retry loop, and the race is the kernel's to
close.
