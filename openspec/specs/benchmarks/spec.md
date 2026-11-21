# benchmarks Specification

## Purpose
Defines how Rhizo's performance is measured, compared across virtual machine
monitors and against existing sandboxes, recorded as a baseline, and protected
from regressions.

## Requirements

### Requirement: Benchmark command
`cargo xtask bench` SHALL build the release kernel, boot it repeatedly on every
supported VMM available on the host, and report per VMM the median and 90th
percentile of the time to ready and of the in-kernel boot cycles. VMMs that are
unavailable SHALL be reported as skipped, not as failures.

#### Scenario: Comparing VMMs
- **WHEN** `cargo xtask bench` runs on a host with KVM, QEMU, Firecracker and Cloud Hypervisor
- **THEN** it prints one row per VMM with median and p90 time to ready and boot cycles

#### Scenario: Missing VMM
- **WHEN** a VMM binary or KVM is not available
- **THEN** that VMM is listed as skipped with the reason and the command still succeeds

### Requirement: Machine-readable results
The benchmark command SHALL write its results as JSON including the metric
name, value, unit, VMM, environment label and number of runs.

#### Scenario: Results file
- **WHEN** `cargo xtask bench --output results.json` completes
- **THEN** `results.json` contains one entry per metric and VMM with those fields

### Requirement: Deterministic metrics
Alongside timings, the benchmarks SHALL record metrics that do not depend on
machine noise: kernel image size and the number of page-table frames used by
the kernel address space.

#### Scenario: Image size
- **WHEN** the benchmarks run
- **THEN** the results include the release kernel image size in bytes

### Requirement: Baseline and regression gate
The repository SHALL store a baseline of benchmark results per environment.
`cargo xtask bench --check` SHALL fail when a metric is worse than its baseline
by more than the threshold configured for that metric and environment, and
SHALL print which metric regressed and by how much.

#### Scenario: Regression detected
- **WHEN** a change makes the time to ready worse than the baseline by more than its threshold
- **THEN** `cargo xtask bench --check` exits with a non-zero status naming the metric and the regression

#### Scenario: Within threshold
- **WHEN** every metric is within its threshold
- **THEN** `cargo xtask bench --check` succeeds

### Requirement: Noise-aware thresholds
Thresholds SHALL be strict for deterministic metrics and wide for timings
measured on shared CI runners. A separate environment for a dedicated machine
with KVM SHALL use tight timing thresholds.

#### Scenario: Shared CI runner
- **WHEN** the benchmark gate runs in the `ci` environment
- **THEN** timing metrics use the wide thresholds stored for `ci`

### Requirement: Benchmark gate in CI
Every pull request SHALL run the benchmark gate in the `ci` environment, and a
failing gate SHALL fail the pull request.

#### Scenario: Slow pull request
- **WHEN** a pull request regresses a metric beyond its `ci` threshold
- **THEN** the CI run for that pull request fails

### Requirement: Comparison chart in the README
`cargo xtask bench --readme` SHALL regenerate a chart in the README comparing
Rhizo's time to ready with a reference sandbox boot measured on the same
machine, and SHALL state the environment and date of the measurement.

#### Scenario: Regenerated chart
- **WHEN** `cargo xtask bench --readme` completes
- **THEN** the README section between the benchmark markers contains a chart, the measured values, the environment label and the date

### Requirement: Suspend and resume metrics
`cargo xtask bench` SHALL report, for a cell with 8 MiB of touched memory,
the time to suspend it, the time to resume it lazily until it can run, the
time to resume it eagerly, the time to resume it lazily and touch every
page, and the snapshot size in bytes.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs on a VMM where the counter frequency is known
- **THEN** the results contain `cell.suspend_us`, `cell.resume_us`, `cell.resume_eager_us`, `cell.resume_touch_us` and `cell.snapshot_bytes`

### Requirement: Fork and template metrics
`cargo xtask bench` SHALL report the time to fork a cell with 50 MiB of
memory, the page-table frames that fork allocates, the time to fork and
then write every page, and the time to spawn the `hello` program from a
template.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs on a VMM where the counter frequency is known
- **THEN** the results contain `cell.fork_us`, `cell.fork_pt_frames`, `cell.fork_touch_us` and `cell.spawn_from_template_us`

### Requirement: Real workload metrics
`cargo xtask bench` SHALL report the time for a real program with an
expensive warm-up to answer one request when started from its executable
and when forked from a template frozen after the warm-up.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs on a VMM where the counter frequency is known
- **THEN** the results contain `workload.cold_us` and `workload.warm_us`

### Requirement: Comparison table
The README benchmark section SHALL include a table of cell operations next
to the Firecracker and Linux-process measurements of the same run, showing
"—" for anything not measured in that run.

#### Scenario: Missing measurement
- **WHEN** the results have no Firecracker restore figure
- **THEN** the Firecracker column of the resume row shows "—"

### Requirement: Complete results from CI
The CI benchmark job SHALL publish its complete results, so the README can
be regenerated from them exactly.

#### Scenario: Large results
- **WHEN** the results JSON is larger than one annotation
- **THEN** it is published in numbered pieces that concatenate to the file

### Requirement: File metrics
`cargo xtask bench` SHALL report the time for a cell to create, write and
read back 1 MiB of file data, and the time to fork a cell holding 16 MiB of
files.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs on a VMM where the counter frequency is known
- **THEN** the results contain `cell.file_write_read_us` and `cell.fork_with_files_us`

### Requirement: Robust cell figures
Each cell figure of a `bench-cell` run SHALL be the minimum, over several
rounds, of the median of each round.

#### Scenario: One slow round
- **WHEN** one round of a cell benchmark is slowed down by its host
- **THEN** the reported figure is that of the fastest round

### Requirement: File metrics are gated
The `ci` baseline SHALL include `cell.file_write_read_us` and
`cell.fork_with_files_us` for every VMM that measures cells, so the bench
gate fails a pull request that makes them slower beyond the threshold.

#### Scenario: Slower file input and output
- **WHEN** a change makes `cell.file_write_read_us` slower than its `ci` baseline by more than the timing threshold
- **THEN** `cargo xtask bench --env ci --check` fails and names the metric

### Requirement: Density metrics
`cargo xtask bench` SHALL report `cell.idle_bytes`, the memory each of 1000
idle cells forked from a template costs, and `cell.suspended_bytes`, the
size of an idle cell's snapshot, as deterministic metrics.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs a `bench-cell` kernel
- **THEN** the results contain `cell.idle_bytes` and `cell.suspended_bytes`

### Requirement: Python workload metrics
Where MicroPython is built, `cargo xtask bench` SHALL report the time for a
MicroPython cell to answer one request started cold from its executable
(`workload.python_cold_us`) and forked from a warm template
(`workload.python_warm_us`).

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs with MicroPython built
- **THEN** the results contain `workload.python_cold_us` and `workload.python_warm_us`

### Requirement: Thread metrics
`cargo xtask bench` SHALL report `cell.thread_create_us`, the time for a
thread of a cell to create a thread that exits at once and to join it, and
`cell.futex_wake_ns`, the time for one thread to wake another blocked on a
futex until that one runs (half of a ping-pong round trip), for every VMM
that measures cells.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs on a VMM where the counter frequency is known
- **THEN** the results contain `cell.thread_create_us` and `cell.futex_wake_ns`

### Requirement: Network metrics
`cargo xtask bench` SHALL report `cell.pipe_rtt_ns`, the time for a cell to
write one byte into a pipe and read it back, `cell.loopback_rtt_us`, the
round-trip time of a 64-byte message between two cells over TCP on
127.0.0.1, and `cell.tcp_resume_us`, the time to resume a cell suspended
with an established loopback TCP connection until it has made one more
64-byte round trip over it and exited, as robust cell figures.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs a `bench-cell` kernel where the counter frequency is known
- **THEN** the results contain `cell.pipe_rtt_ns`, `cell.loopback_rtt_us` and `cell.tcp_resume_us`

### Requirement: CPython workload metrics
Where CPython is built, `cargo xtask bench` SHALL report the time for a
CPython cell to answer one agent request started cold from its executable
(`workload.cpython_cold_us`: interpreter start, imports, warm-up and
answer) and forked from a template frozen after the warm-up
(`workload.cpython_warm_us`), for every VMM that measures cells.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs with CPython built
- **THEN** the results contain `workload.cpython_cold_us` and `workload.cpython_warm_us` for guest `rhizo`

### Requirement: Network card metric
Where the VMM gives the kernel a network card and a host echo server,
`cargo xtask bench` SHALL report `net.tcp_rtt_us`, the round-trip time of a
64-byte message between a cell and a TCP echo server on the host; elsewhere
it SHALL report the metric as skipped with the reason.

#### Scenario: Metric present
- **WHEN** `cargo xtask bench` runs on QEMU `microvm` with KVM
- **THEN** the results contain `net.tcp_rtt_us/rhizo/qemu-microvm`

#### Scenario: Metric on PCI VMMs
- **WHEN** `cargo xtask bench` runs on QEMU `q35`, and on Cloud Hypervisor with password-less `sudo` for its tap device
- **THEN** the results contain `net.tcp_rtt_us/rhizo/qemu-q35` and `net.tcp_rtt_us/rhizo/cloud-hypervisor`

### Requirement: Multi-core metrics
`cargo xtask bench` SHALL also boot the `bench-cell` kernel with 4 vCPUs on
every VMM that measures cells and report `cell.parallel_us` (time per
CPU-bound cell when four such cells per CPU run on every CPU),
`cell.scaling_pct` (that batch's time on every CPU, times the number of
CPUs, over its time on one CPU, in percent: 100 is perfect scaling) and
`cell.ipi_wake_us` (handing work to an idle CPU, waking it with an
interrupt, until it is done).

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs on a VMM where the counter frequency is known
- **THEN** the results contain `cell.parallel_us`, `cell.scaling_pct` and `cell.ipi_wake_us`

### Requirement: Parallel thread metrics
The 4-vCPU `bench-cell` boot of `cargo xtask bench` SHALL also report
`cell.thread_scaling_pct` (the time of `parsum` with 4 threads, times 4,
over its time with 1 thread, in percent: 100 is perfect scaling) and
`cell.shootdown_us` (one `munmap` of a written page by a thread of a cell
whose other threads run on the other CPUs), on every VMM that measures
cells. A kernel with one CPU SHALL measure the thread metrics of
requirement "Thread metrics" on the single-CPU path, as before.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs on a VMM where the counter frequency is known
- **THEN** the results contain `cell.thread_scaling_pct` and `cell.shootdown_us`

### Requirement: Laptop workload
`cargo xtask bench` SHALL run a laptop workload on every VMM that gives
the kernel a network card: an `agent` cell frozen as a template after its
warm-up, with a shared folder seeded into its files, 50 agents opened from
it of which 45 are then suspended, and a fake model server on the host
answering HTTP requests after a configurable delay. Every agent SHALL
still be live at the end.

#### Scenario: Agents opened and suspended
- **WHEN** `cargo xtask bench --workload laptop` runs on QEMU `microvm`
- **THEN** 50 agents open from one template, 45 are suspended and resumed, each live one answers turns, and all 50 are live at the end

#### Scenario: No network card
- **WHEN** the VMM gives the kernel no network card or no model server
- **THEN** the laptop workload is listed as skipped with the reason

### Requirement: Laptop metrics
The laptop workload SHALL report `laptop.open_us` (fork until the agent
opened its session), `laptop.suspend_us`, `laptop.resume_us` (lazy resume
until runnable), `workload.agent_turn_us` (a user message until the agent
asked the model, got its answer and ran its tool), `laptop.resume_turn_us`
(the same right after a resume) and `laptop.memory_bytes` (memory the
template and the 50 agents hold, snapshots included).

#### Scenario: Metrics present
- **WHEN** the laptop workload runs on a VMM with a network card
- **THEN** the results contain each of these metrics for that VMM

### Requirement: Idle cost on a laptop
Over a window in which every agent waits, the laptop workload SHALL report
`laptop.idle_wakeups_per_s` (returns of the guest's CPUs from waiting for
an interrupt, per second) and `laptop.idle_host_cpu_pct` (CPU time of
the VMM process on the host over wall time, read from `/proc`).

#### Scenario: Idle window
- **WHEN** the guest prints the marks that open and close its idle window
- **THEN** the host reads the VMM's CPU time at each and reports both metrics

### Requirement: Server workload
`cargo xtask bench` SHALL run a server workload on every VMM: up to 10,000
cells forked from `worker` templates in a fixed mix (computing, writing and
reading files, pairs talking over loopback TCP), each running one
operation per period for the load's duration, while the host suspends and
resumes one worker and forks one at a constant rate. Fewer cells than the
target SHALL be made only when memory leaves too little headroom, with
that reason in the output. Every worker SHALL end with status 0.

#### Scenario: Ten thousand cells
- **WHEN** `cargo xtask bench --workload server` runs on QEMU `microvm` with 1 GiB of memory
- **THEN** 10,000 workers run and each exits with 0

#### Scenario: A worker fails
- **WHEN** a worker ends with a status other than 0 or a fault
- **THEN** the run fails and names the worker and what it printed

### Requirement: Server metrics
The server workload SHALL report `density.cells` (cells made),
`density.ops_per_s`, `density.cell_bytes`, and the 50th, 99th and 99.9th
percentiles of scheduling latency (joined a run queue to running,
`density.sched_*_us`), wake-up latency (deadline or socket change to
running, `density.wake_*_us`) and system-call latency
(`density.syscall_*_ns`).

#### Scenario: Metrics present
- **WHEN** the server workload runs
- **THEN** the results contain `density.cells`, `density.wake_p99_us` and the other server metrics for that VMM

### Requirement: Clean failure when memory runs out
After the server workload, the kernel SHALL fork idle cells until it
refuses one and report how many it held (`density.oom_cells`). Then
spawning from an executable SHALL fail with an out-of-memory error,
forking a live cell and an eager resume SHALL be refused, a cell mapping
memory SHALL receive `ENOMEM`, and the kernel SHALL NOT panic; once every
cell ended, every frame and heap allocation SHALL be back. A run in which
any of this does not hold SHALL fail the benchmark.

#### Scenario: Memory full of cells
- **WHEN** the server workload's out-of-memory push runs
- **THEN** each refusal happens, `hog` prints `exhausted`, nothing leaks, and the benchmark succeeds

#### Scenario: Unclean failure
- **WHEN** the out-of-memory line reports a refusal that did not happen or leaked bytes
- **THEN** `cargo xtask bench` fails and prints that line

### Requirement: Long workloads weekly
`cargo xtask bench --soak` SHALL run the long versions of the workloads
(the server load for 10 minutes, a 60-second laptop idle window), and a
scheduled weekly CI job SHALL run them and publish the results.

#### Scenario: Soak
- **WHEN** the weekly workflow runs
- **THEN** it runs `bench --workload server --soak` and `bench --workload laptop --soak` and publishes their results

### Requirement: Metrics where more is better
For `density.cells`, `density.ops_per_s` and `density.oom_cells`, higher
SHALL be better: `cargo xtask bench --check` SHALL fail when the baseline
exceeds the value by more than the metric's threshold.

#### Scenario: Fewer cells
- **WHEN** `density.cells` drops more than 1 % below its baseline
- **THEN** `cargo xtask bench --check` fails and names the metric

### Requirement: Dynamic linking metrics
`cargo xtask bench` SHALL report, for every VMM that measures cells, the
time to start the dynamically linked `hello` from the cell's files and run
it to its exit (`workload.dynamic_run_us`), and the time for the same
program forked from a template frozen once its interpreter has started it
(`workload.dynamic_template_us`), to compare with a static `hello`
(`cell.run_us`).

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs on a VMM where the counter frequency is known
- **THEN** the results contain `workload.dynamic_run_us` and `workload.dynamic_template_us` for guest `rhizo`

### Requirement: Dynamic linking on Linux
`cargo xtask bench` SHALL report the time to spawn the same dynamically
linked `hello`, with the same interpreter and `libc`, as a Linux process
until it exits, in the comparison's Firecracker guest and on the host
(`workload.dynamic_run_us` for guest `linux-process`).

#### Scenario: Linux process
- **WHEN** `cargo xtask bench` runs where Firecracker can boot the reference Linux guest
- **THEN** the results contain `workload.dynamic_run_us` for guest `linux-process` and VMM `firecracker`

### Requirement: Missing figures fail the gate
`cargo xtask bench --check` SHALL fail when a gated metric stored in the
baseline for a VMM was not measured although that VMM produced other
figures in the run, naming the metric and the skipped steps with their
reasons.

#### Scenario: Cell benchmark crashed
- **WHEN** the cell benchmark VM crashes on a VMM and its figures are skipped
- **THEN** the gate fails, naming each missing metric and the crash

#### Scenario: VMM not run
- **WHEN** a run measures only some VMMs
- **THEN** figures of the VMMs it did not run are not reported missing

### Requirement: Room for the fork benchmark
The cell benchmark VM SHALL have enough memory for a 50 MiB cell and a
child that writes every page of it, beside the kernel image with every
benchmark program.

#### Scenario: Fork of a 50 MiB cell
- **WHEN** the cell benchmarks run on any VMM
- **THEN** the fork benchmark's child exits successfully and the fork figures are measured

### Requirement: Agent metrics
`cargo xtask bench` SHALL report, where the CPython agent can run, one of
its turns as the agent times it, the first left out
(`workload.agent_cpython_turn_us`) and its part spent on the model's answer
(`workload.agent_cpython_model_us`), a suspend plus eager resume between two turns
(`workload.agent_suspend_resume_us`) and a fork from its decision point
until its three branches run (`workload.agent_fork_us`); elsewhere it SHALL
list them as skipped with the reason.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs on Firecracker with KVM and CPython built
- **THEN** the results contain `workload.agent_cpython_turn_us`, `workload.agent_suspend_resume_us` and `workload.agent_fork_us` for guest `rhizo` on `firecracker`

#### Scenario: No model server
- **WHEN** the VMM gives the kernel no network card
- **THEN** the agent metrics are listed as skipped with the reason

### Requirement: Control-plane metrics
`cargo xtask bench` SHALL report, on every VMM it measures, the round trip
of a `list` request from the host through a control kernel and back
(`control.roundtrip_us`), and the host-measured time of `run`, `suspend`,
`resume`, `fork` and `kill` on an idle cell (`control.run_us`,
`control.suspend_us`, `control.resume_us`, `control.fork_us`,
`control.kill_us`); a VMM where the control kernel cannot run SHALL be
listed as skipped with the reason.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs on QEMU `microvm`
- **THEN** the results contain `control.roundtrip_us` and the five operation times for guest `rhizo` on `qemu-microvm`

### Requirement: AArch64 instruction gate in CI
Every pull request SHALL count the AArch64 kernel's benchmarks in retired
instructions under QEMU with `-icount` (`cargo xtask bench --icount-only`)
on a GitHub-hosted ARM64 runner, and SHALL fail when a gated count is
worse than its baseline in the `ci-arm` environment by more than the
instructions threshold. The results SHALL be published under annotation
titles distinct from the x86_64 job's.

#### Scenario: AArch64-only regression
- **WHEN** a pull request adds more than 2 % of instructions to a gated operation on AArch64 only
- **THEN** the AArch64 CI job fails naming the `qemu-virt-icount` metric

#### Scenario: Same operations on both architectures
- **WHEN** the AArch64 kernel runs under `-icount` with performance counters in the guest
- **THEN** the results hold the fork benchmarks too, counted with the clock where no counter pass covers them

### Requirement: Both architectures in the README
`cargo xtask bench --readme` SHALL accept several results files and merge
them, refusing a metric present in more than one. When AArch64 instruction
counts are present, the README section SHALL show them next to x86_64's;
without them the section SHALL be unchanged.

#### Scenario: Merged results
- **WHEN** `cargo xtask bench --from x86_64.json --from aarch64.json --readme` runs
- **THEN** the README section holds a table of instructions per operation with an x86_64 and an AArch64 column

### Requirement: Interactive turn metric
`cargo xtask bench` SHALL report, on every VMM where the control kernel
runs and CPython is built, `chat.turn_us`: the Rhizo side of one
interactive turn of the chat agent against the host's fake model, from the
host's `input` request until the agent's answer is read back, less the
time the agent waited for the model.

#### Scenario: Metric present
- **WHEN** `cargo xtask bench` runs on QEMU `microvm` with CPython built
- **THEN** the results contain `chat.turn_us` for guest `rhizo` on `qemu-microvm`

### Requirement: System-call scaling metric
The 4-vCPU `bench-cell` boot SHALL report `cell.syscall_scaling_pct`: the
time of four `piper` cells per CPU (each writing and reading a byte
through its pipe 20,000 times) run on every CPU, times the number of
CPUs, over their time run on one CPU after the other, in percent (100 is
perfect scaling).

#### Scenario: Metric present
- **WHEN** `cargo xtask bench` runs on a VMM where the counter frequency is known
- **THEN** the results contain `cell.syscall_scaling_pct`

### Requirement: Timer accuracy metric
The 4-vCPU boot of `cargo xtask bench` SHALL report `cell.timer_error_us`:
on each secondary CPU, the median of five 10 ms waits on that CPU's own
timer, its distance from 10 ms by the counter, the worst CPU's.

#### Scenario: Metric present
- **WHEN** `cargo xtask bench` runs on a VMM where the counter frequency is known
- **THEN** the results contain `cell.timer_error_us`

### Requirement: Node.js workload metrics
Where Node.js is packed, `cargo xtask bench` SHALL report the time for a
Node.js cell (`--jitless`) to answer one agent request started cold from
its files (`workload.node_cold_us`: interpreter, start-up, warm-up, answer,
exit) and forked from a template frozen after the warm-up
(`workload.node_warm_us`), for every VMM that measures cells, from a
kernel and VM of their own; `--workload node` SHALL measure them alone.
Where it is not packed, they SHALL be listed as skipped with the reason.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs with `RHIZO_NODE` set
- **THEN** the results contain `workload.node_cold_us` and `workload.node_warm_us` for guest `rhizo`

#### Scenario: Not packed
- **WHEN** `cargo xtask bench` runs without `RHIZO_NODE`
- **THEN** `workload.node_*` is listed as skipped with the reason

### Requirement: Wasted wake-up metric
The server workload SHALL report `density.wasted_wakeups_pct`: of the cells
woken from the waiting set by a socket change during the load, the share,
in percent, whose next system call waited on descriptors again. Lower is
better.

#### Scenario: Metric present
- **WHEN** the server workload runs
- **THEN** the results contain `density.wasted_wakeups_pct` for that VMM, computed from the `io_woken` and `io_wasted` counts of its result line

### Requirement: Server latencies after the start
The server workload's scheduling and wake-up latencies SHALL count from the
first moment after its load starts at which no cell runs or waits for a
CPU: before it, the workers made ahead of the load all run for the first
time. A load that never reaches such a moment SHALL count them from its
start. The breakdown line SHALL report when that moment came as
`settle_ns` (0: never).

#### Scenario: Cells runnable at the start
- **WHEN** counting starts with a cell that yields 2,000 times and four cells that sleep 250 ms
- **THEN** the yields are not counted, and the four wake-ups and their scheduling are

#### Scenario: Counting from the start
- **WHEN** the same cells run without waiting for that moment
- **THEN** the yields are counted too

### Requirement: Server tail latencies are gated
The `ci` baseline SHALL include `density.sched_p99_us` and
`density.wake_p99_us` for every VMM that runs the server workload, so the
bench gate fails a pull request that makes them slower beyond the timing
threshold.

#### Scenario: A longer wake-up tail
- **WHEN** a change makes `density.wake_p99_us` slower than its `ci` baseline by more than the timing threshold
- **THEN** `cargo xtask bench --env ci --check` fails and names the metric
