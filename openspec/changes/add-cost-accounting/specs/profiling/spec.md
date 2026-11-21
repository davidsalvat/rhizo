## ADDED Requirements

### Requirement: Cost accounting
A kernel built with the `profile` feature SHALL charge each CPU's time, in
counter ticks, to one part at a time: cell code (with the kernel's entry and
exit path), system calls, page faults, copy-on-write faults, the frame
allocator, copies, the network, the scheduler, idle, lock waits, cell
operations, teardown, or the rest of the kernel. A part nested in another
SHALL be excluded from it, so the parts add up to the time accounted.

#### Scenario: A cell's cost adds up
- **WHEN** a cell making 100 `getpid` calls runs with every part accounted
- **THEN** cell code, system calls, spawn, teardown, the frame allocator and copies each have time, and the parts add up to no more than the time elapsed

### Requirement: Cost per system call
While accounting runs, the kernel SHALL record each system call's time and
count under its number, and charge socket calls to the network part.

#### Scenario: Calls by number
- **WHEN** a cell making 100 `getpid` calls runs with every part accounted
- **THEN** at least 101 system calls were entered and 100 of them are recorded as `getpid`

### Requirement: Accounting costs nothing unless built in
Accounting SHALL be started for a chosen set of parts, the others' hooks
doing nothing. Without the `profile` feature the hooks SHALL compile to
nothing, and the benchmarks SHALL run without them.

#### Scenario: No cost when not built in
- **WHEN** the `bench-cell` kernel is built without `profile` and run under `-icount`
- **THEN** its instruction counts are those of a kernel without the hooks

### Requirement: Workload breakdowns
`cargo xtask bench` SHALL run a `profile` kernel on every available VMM and
publish `breakdown.<workload>.<part>_us` for the cell benchmark loop
(`cell_loop`), a fork writing every page of 50 MiB (`fork_touch`), the
CPython cold start (`cpython_cold`, where CPython is built) and sixteen
CPU-bound cells on four CPUs (`parallel`). Breakdown metrics SHALL NOT be
gated.

#### Scenario: CPython cold start
- **WHEN** `cargo xtask bench` runs where CPython is built
- **THEN** the results hold `breakdown.cpython_cold.<part>_us` for every part, `total` included

### Requirement: Cell time split into user code and entry and exit
A breakdown SHALL split cell time into `user` and `entry_exit`: the number
of entries into cell code times the cost of one entry, measured on a cell
that only makes system calls, and never more than the cell time. The kernel
SHALL also print the scopes entered per part and the system calls that took
most time.

#### Scenario: A cell that only prints
- **WHEN** the cell benchmark loop is broken down
- **THEN** its `entry_exit` and `user` add up to its cell time

### Requirement: Lock contention
Every kernel spinlock SHALL count the acquisitions that found it held by
another CPU and the times the waiting CPU looked at it again, touching these
counters only on the waiting path. `cargo xtask bench` SHALL publish
`lock.<name>_waits` for the four-CPU workload, and its waiting time SHALL be
the `lock` part of its breakdown.

#### Scenario: A held lock
- **WHEN** one CPU holds a lock while another asks for it
- **THEN** the lock reports one wait and at least one spin

### Requirement: Performance counters
The kernel SHALL find the CPU's performance counters through the `arch`
interface (`pmu`): Intel's architectural performance monitoring or AMD's
core counters on x86_64, PMUv3 on AArch64, counting retired instructions and
core cycles in kernel and cell code. A guest without them SHALL be reported
(`rhizo-pmu none`).

#### Scenario: Counters advance
- **WHEN** the in-kernel tests run where the guest has counters
- **THEN** a loop of 10,000 iterations counts cycles and, where instructions are counted, at least 10,000 instructions

### Requirement: Benchmarks in instructions and cycles
Where the guest has counters, the `bench-cell` kernel SHALL run its
single-CPU benchmarks again with the PMU as their clock, once counting
instructions and once cycles, and `cargo xtask bench` SHALL publish each
timed figure as `<metric>_insns` and `<metric>_cycles`; without counters
these SHALL be listed as skipped.

#### Scenario: Guest with counters
- **WHEN** the benchmarks run in a guest whose PMU counts instructions
- **THEN** the results hold `cell.switch_insns` and `cell.switch_cycles` for that VMM next to `cell.switch_ns`

#### Scenario: Guest without counters
- **WHEN** the guest has no performance counters
- **THEN** the counted metrics are listed as skipped for that VMM and the other metrics are unaffected

### Requirement: Instruction counts under icount
`cargo xtask bench` SHALL run the `bench-cell` and `profile` kernels once
each on the default QEMU machine with `-icount shift=0` (the guest's clock
counts its instructions) and publish every timed figure and breakdown as
`<metric>_insns/rhizo/<vmm>-icount`, gated with their own threshold
(`thresholds_pct.instructions`, 2 % in `ci`).

#### Scenario: Two runs agree
- **WHEN** the same kernel runs twice under `-icount`
- **THEN** every `_insns` figure is the same in both runs

#### Scenario: Instruction regression
- **WHEN** a change adds more than 2 % of instructions to a gated operation
- **THEN** the `ci` gate fails for that metric

### Requirement: System call and copy-on-write metrics
The benchmarks SHALL measure `cell.syscall_ns`, the round trip of one
`getpid` call (a cell making 20,000 calls, minus the same cell making none),
and `cell.cow_fault_ns`, the kernel's time resolving one copy-on-write fault
of a 4 KiB page, accounted alone while a fork of a cell holding 1 MiB
writes every page.

#### Scenario: Round trip
- **WHEN** `cargo xtask bench` runs
- **THEN** the results hold `cell.syscall_ns` per VMM and `cell.syscall_insns` under `-icount`
