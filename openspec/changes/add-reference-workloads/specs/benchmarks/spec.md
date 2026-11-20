# Spec Delta

## ADDED Requirements

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
