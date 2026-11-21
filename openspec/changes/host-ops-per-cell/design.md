# Design

## D1. Host operations from the run's stop condition

`Host::run(stop)` already calls `stop` on the boot CPU between two of its
own slices, holding no cell, while the secondary CPUs run theirs. `stop`
now gets `&mut Host`, and every host operation (suspend, resume, fork,
freeze, kill, spawn, spawn from a template, wake, input, quotas, output,
writing a file) may be called there. The other CPUs keep running cells; an
operation stops only the CPUs that run the cell it acts on (D2). Called
between runs, the operations behave as before.

| Option | Global stop | Change |
|---|---|---|
| **Operations from `stop`, on the boot CPU (chosen)** | none | `stop` gets `&mut Host`; operations lock instead of `get_mut` |
| Secondary CPUs that outlive `run` | none | the host's shared state must not move (`Host` is moved by value in benchmarks), and every caller that expects nothing to run after `run` returns (most in-kernel tests) changes |
| A queue of requests that a secondary CPU executes | none | a second path for every operation, results sent back, and a CPU taken from cells |

Ending a run still stops every CPU: that is how a caller asks for every
cell to be quiet (tests that compare outputs after a run, `Drop`). The
server workload (`density.*`) makes its suspend, resume, fork and kill from
`stop` now.

## D2. Taking one cell: claim, interrupt only its CPUs, wait

Every operation starts by taking its cell out of the scheduler under the
host's lock (`Host::take_cell`); queries that only read or change a few
fields (`output`, `write_file`, `set_quota`...) act on the cell where it is,
under that lock, when it is in the scheduler (`Host::with_live`). A cell in
no queue is either running or not live. To tell them apart, the host's
shared state records the cell each CPU runs (`Shared::on_cpu`), set when a
CPU takes a cell or joins a gang and cleared when it gives it back or
leaves; the last CPU to leave a gang keeps its entry until it has given
the cell back, so the cell is always in exactly one of: the scheduler, a
CPU's entry, the claim slot, or gone.

If CPUs run the cell, the host stores its identifier in `CLAIM` (an atomic:
identifiers are never reused, so one word serves every host), interrupts
exactly those CPUs (`smp::wake`, the wake-up interrupt), and spins,
taking the lock between looks, until the cell is in `Shared::claimed` or
no CPU runs it any more (it ended: the operation fails, as for a cell that
is not live). A CPU sees the claim

- on the single-CPU path, at the interrupt (the wake-up interrupt arrives as
  `Trap::Interrupt`; one more compare where the slice's tick count is
  already compared), and ends the slice;
- on the gang path, before it enters the cell's code (a load per entry),
  and leaves after putting its thread back; a claimed gang offers nothing,
  so no idle CPU joins it meanwhile;
- when it gives the cell back for any reason (`give_back`, under the lock):
  the cell goes into `Shared::claimed` instead of a queue.

An interrupt that arrives while the CPU is in kernel code stays pending
and is taken at the next entry into cell code, so a claim waits at most for
the system call or fault in progress. A cell that ends instead ends as
usual. The operation then puts the cell back (runnable, or filed as its
threads say) and wakes an idle CPU if any cell is runnable
(`Host::admit`); taking a cell does the same, since the boot CPU, working
for the host, does not take the cells it left runnable.

Lost: a reader-writer lock on cells (every slice would take it); stopping
gangs thread by thread (D1 of `add-parallel-threads` keeps the cell as the
unit); sending the operation to the CPU that runs the cell (it would run a
host operation with the cell's tables loaded and the cell's lock order).

## D3. What an operation shares with running cells

The cell the operation acts on runs nowhere while it does (D2). What it
shares with other cells, and why each is safe without a global stop:

| State | Protection | Why it holds |
|---|---|---|
| Scheduler, tags, `on_cpu`, claim slot | the host's lock | held only to find, take and file cells, never across an operation's work (copying tables, capturing a snapshot) |
| Copy-on-write tables and frames shared with running forks | `SHARING` around snapshot capture and teardown, as for cells' own calls and faults (D6 of `add-smp-scheduling`) | unchanged |
| Fork (`fork_lower_half`), spawn from a template | none | fork writes only the parent's root, which nobody else reaches, and only raises counts of tables the parent holds. A cell drops or copies a shared table only through its own reference, so while the parent holds one the count is at least two on both sides: a count going up never meets a decision that a count going down makes wrong. A discarded child (fork refused half-way) drops references, so it takes `SHARING` |
| Frame reference counts (`SHARED`) | their own lock, per change | each change is atomic; the order argument is the row above |
| Page cache, image cache | their own locks | cells already use them from other CPUs (`exec`, file mappings; "another CPU loaded it meanwhile") |
| Kernel heap | the allocator's lock | cells already end, and trim the heap, on other CPUs. Page tables retired from the heap window are still freed only when every secondary CPU idles (D5 of `add-heap-trim`), so during a long run they wait for its end: rare, as before |
| Sockets (`NET`) | their lock; order host, then sockets | unchanged |
| TLBs | the cell's clean set and tag | the cell runs nowhere: a fork's flush on the boot CPU with the clean set reduced to it (x86_64) or a broadcast (AArch64), and freed tags drop their translations on their next owner's first entry on each CPU, as between runs. Other cells' translations do not change: an operation only changes its own cell's root and counts |
| FP/SIMD state | — | with several CPUs a cell's state leaves the CPU with the cell (spec `multi-core`) |

Lock order stays: cell, `SHARING`, host, sockets. The host never takes a
cell's lock: it waits for the gang to end.

## D4. Finding a cell: keys next to the entries

Lookups by identifier scanned every queue, the waiting set and the sleep
list, calling a predicate that read each cell (one pointer chase per cell,
10,000 at the server workload's target), under the lock every CPU takes
to take or give back a cell. The scheduler now keeps each entry's key
(`Keyed::key`, the cell's identifier, read once as the entry is added)
next to it: `remove`, `find` and `find_mut` take a key, look it up in the
waiting set (a map by key), and otherwise compare keys over the queues and
the sleep list without touching any cell.

| Option | Lookup | Cost per take and give-back |
|---|---|---|
| **Keys next to entries (chosen)** | O(n) compares over contiguous memory | one load of the key, already in cache |
| A map from key to place (queue and arrival number) | O(log n) | a map update on every take and give-back: the switch is 880 instructions (`cell.switch_insns`), gated at +2 % |
| A per-cell slot table | O(1) | a slot allocator and a store per move; more code than the lookups need |

The sleep list becomes a deque. As a sorted vector it lost its due entries
from the front, so every wake-up moved all the other sleepers: 96 KB with
the server workload's 6,000 sleeping workers, and 144 KB once the keys made
each entry 24 bytes instead of 16. That doubled the workload's p99 sched
and wake latencies on one CPU (in CI: 3.5 ms against 1.5 ms). As a deque,
the due entries leave from the front and a new deadline, most often the
latest, joins at the back, and nothing else moves. A take with 6,000
sleepers went from 1.4 µs (main) and 1.9 µs (keys) to 0.05 µs, measured
on the host.

## D5. Bounded cost

The host's lock is held for a key lookup and a constant amount of work per
operation; an operation's own work (copying tables, capturing a snapshot,
freeing a cell) runs outside it and scales with that cell only. A claimed
cell's CPUs stop for the interrupt round trip and the end of their kernel
section; no other CPU stops at all. The host's wait is bounded by the same.

Between runs no other CPU reaches the shared state, so operations use it
without the lock (`Host::locked`), as before this change: forks, spawns
and templates cost what they did (`cell.fork_insns`,
`cell.spawn_from_template_insns`, `cell.spawn_insns`). A CPU giving a cell
back files it and updates its own bookkeeping in one critical section, so
claims add no lock round trip to a switch (`cell.switch_insns`).

## D6. Measuring the stall

`progress` counts in a loop without system calls and keeps, in a word next
to its counter, the longest time between two rounds (counter ticks); the
host reads both through the direct map while it runs (`Host::watch`,
tests and benchmarks only). The 4-vCPU `bench-cell` boot runs `progress`
and a spinner; in each of 21 rounds the host forks the spinner, kills the
child, suspends the spinner and resumes it, and reads the longest gap from
just before the operations until it has counted again after them. The
median is `cell.host_op_stall_us` with the operations made from `stop`,
and `cell.host_op_stall_stopped_us` with each round between runs (every
CPU stopped and started again, as before this change), for comparison.
Both start ungated, like the other multi-core metrics.

## D7. Not changed

- `Host::step` and runs on one CPU: the boot CPU is the only one; nothing
  else could run during an operation anyway.
- The control kernel answers requests between runs: requests come from a
  person at human pace, its loop reads the serial line between runs, and
  moving that loop into `stop` changes its structure for no measured
  gain. It can move later with no kernel change.
- Snapshots, forks and templates: their contents and formats are those of
  a cell taken between runs.
