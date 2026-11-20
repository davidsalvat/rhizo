# Design

## D1. Hooks compiled in only with `profile`

Accounting needs a hook at every transition: entering and leaving cell code,
each system call, each fault, each frame allocated or freed, each copy, each
scheduling step. A hook that is only switched off at run time still costs a
load and a branch, and its code changes what the compiler inlines. Measured
under `-icount` with run-time switches, a switch between cells cost 70
instructions more (+9 %), a pipe round trip +10 %, a warm CPython-style fork
+12 %.

So the hooks are built only with the kernel feature `profile`. Without it,
`prof::scope` returns a zero-sized value with no destructor and the hooks
compile to nothing: the `bench-cell` kernel's instruction counts under
`-icount` are those of the kernel without hooks (within 0.6 %, the rest
being the compiler's own choices, several of them in favour). `ktest`
builds include `profile`, so the in-kernel tests exercise it. `cargo xtask
bench` builds a second kernel, `bench-cell` with `profile`, whose `bench`
prints the breakdowns instead of the benchmarks: the timed figures never run
with hooks in.

Alternative: run-time switch only (rejected: the overhead above). Separate
kernels per workload (rejected: one more build each, nothing gained).

## D2. Exclusive parts on a per-CPU stack

`rhizo_prof::Account` keeps a stack of open parts (eight deep; deeper ones
are folded into their parent) and the tick at which the current part was
entered. `enter` charges the ticks since then to the current part and
pushes; `leave` charges and pops. Every tick goes to exactly one part, so
the parts add up to the time accounted (a property test checks it for any
sequence of calls). Each scope also adds to its part's count, and the
outermost scope of a part adds its whole duration to the part's inclusive
time: that is how `cell.cow_fault_ns` is measured.

Each CPU has its own account behind a plain `SpinLock`, not a `Global`:
`Global` itself calls the lock-wait hook, and only the owning CPU takes the
lock while accounting runs (`stop` merges them at the end). Accounting can
be started for a set of parts (a bit mask); the copy-on-write metric
accounts only that part, so the frame allocator's and copies' hooks inside
the fault cost nothing.

System calls are charged to `syscall`, or to `net` for socket calls
(`rhizo_abi::Arch::is_socket_call`), and recorded per number
(`rhizo_prof::Calls`, 512 slots per CPU; names come from
`rhizo_abi::Arch::call_name`).

Where the hooks sit: around `enter_user` (cell), the system call inside the
`SHARING` and socket locks (so lock waits are their own part), the lazy
page-in and stack growth (fault), `resolve_write` (cow), `KernelFrames`
(frames), `DirectMap`'s byte copies, zeroing and `copy` (copy), `take`,
`give_back` (sched), `wait` (idle), `poll_wire` (net), `Host::spawn*`,
`fork`, `suspend`, `resume`, `freeze`, `spawn_from_template` (spawn), `end`
(teardown), and `Global::with`'s waiting path (lock).

## D3. Entry and exit estimated from a cell that only makes calls

Cell time includes the kernel's entry and exit path (assembly in
`arch::user`); separating them exactly needs timestamps in that assembly,
which every exit would pay. Instead the kernel accounts a cell making
20,000 `getpid` calls: its cell time per entry is almost only the entry and
exit path, and each workload's `entry_exit` is its number of entries times
that cost (never more than its cell time). `user` is the rest.

## D4. Contention counted on the waiting path only

`SpinLock::with` keeps its single compare-and-swap when the lock is free;
a lock found held goes to a `#[cold]` function that panics on reentrance,
spins with plain loads, and adds to two atomic counters (`waits`, `spins`).
`with_waiting` also calls a closure when it starts waiting and another when
it gets the lock, which `profile` builds use to open and close a `lock`
scope. Under `-icount` the change costs nothing (the first version, with the
waiting state in the fast path, cost 12 instructions per switch).

## D5. Performance counters as a second clock for the same code

`arch::pmu` programs general-purpose counters 0 and 1 on x86_64 (Intel
architectural events `INST_RETIRED.ANY_P` and `CPU_CLK_UNHALTED.THREAD_P`
when CPUID leaf 0xA reports them; AMD events 0xC0 and 0x76 when
`PerfCtrExtCore` is set, with `PerfMonV2`'s global enable when present) and
PMUv3's cycle counter and event counter 0 (`INST_RETIRED`, when `PMCEID0`
reports it; a 32-bit counter is extended to 64 bits in software) on
AArch64, counting in kernel and user mode. Hypervisors that give the guest
no counters report none, and nothing is programmed.

The figures in instructions and cycles come from the same benchmark code:
the benchmarks read `bench_clock()` (the timer, or a PMU counter) and print
through `bench_ns()`, and `bench` runs them again with each PMU clock after
a `rhizo-pass` line. A first version measured six operations in a separate
function; its new call sites changed inlining (`Host::resume` was no longer
inlined into its only caller) and the timed figures got 2 to 3 % slower
under `-icount`. Reusing the code avoids that, and gives every metric, not
six, a count. Reading a counter under KVM is a VM exit, which costs time but
not guest instructions, so the passes never run while time is measured.
The passes skip the fork benchmarks (`cell.fork`, `fork_touch`,
`spawn_from_template`): forking 50 MiB and writing it needs most of the
guest's memory, and the heap the first pass grew keeps some (seen in CI:
the child faulted for want of frames). `-icount` counts those.

## D6. `-icount` as the deterministic instruction counter in CI

The CI runners are shared virtual machines; whether their KVM guests get a
PMU depends on the cloud host (often not). QEMU's TCG with `-icount
shift=0,sleep=off` advances the guest's clock by one nanosecond per
instruction executed and jumps over idle time instead of following the
host: a kernel timing itself with its counter measures retired
instructions, and two runs of the same kernel print the same numbers (the
benchmarks run without the network card there, the only input from the
host). On AArch64, QEMU's PMU counts instructions only under `-icount`, and
the instruction pass is used when present (the generic timer ticks once per
16 instructions there).

These counts are gated at 2 % (`thresholds_pct.instructions`), not 1 %
like other deterministic metrics: a toolchain or dependency update moves
them a little. Breakdowns (`breakdown.`) and lock waits (`lock.`) are
recorded but not gated (`ungated` entries ending in `.` are prefixes).

## D7. Copy-on-write copies in one call

`rhizo-mm` copied a page through a 512-byte stack buffer, sixteen calls per
page. `PhysicalMemory::copy(from, to, len)` (default: the same loop) lets
the kernel's direct map do one `memcpy`: a copy is one accounted event
instead of sixteen, and the fork benchmark that writes 50 MiB executes 30 %
fewer instructions (`cell.fork_touch_insns` 10.0 M → 7.0 M).

## D8. Metrics

| Metric | Where |
|---|---|
| `cell.syscall_ns/rhizo/<vmm>` | `rhizo-ops` line of `bench-cell` |
| `cell.cow_fault_ns/rhizo/<vmm>` | `rhizo-ops` line of the `profile` kernel |
| `<metric>_insns`, `<metric>_cycles` `/rhizo/<vmm>` | PMU passes (`rhizo-pass`), where the guest has a PMU |
| `<metric>_insns/rhizo/<qemu>-icount` | one run of each kernel under `-icount` |
| `breakdown.<workload>.<part>_us/rhizo/<vmm>` | `rhizo-breakdown` lines (median of three runs) |
| `lock.<name>_waits/rhizo/<vmm>` | `rhizo-locks` line of the four-CPU run |
