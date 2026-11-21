## MODIFIED Requirements

### Requirement: Benchmark gate in CI
Every pull request that changes code SHALL run the benchmark gate on the
instruction counts under QEMU with `-icount` only, on both architectures
(`cargo xtask bench --icount-only --check`), each count measured once, and
a failing gate SHALL fail the pull request. Timings SHALL NOT gate pull
requests.

#### Scenario: Slow pull request
- **WHEN** a pull request adds more instructions to a gated operation than its threshold allows
- **THEN** the CI run for that pull request fails naming the `-icount` metric

### Requirement: Complete results from CI
The benchmark run on main SHALL publish its complete results, so the
README can be regenerated from them exactly.

#### Scenario: Large results
- **WHEN** the results JSON is larger than one annotation
- **THEN** it is published in numbered pieces that concatenate to the file

### Requirement: AArch64 instruction gate in CI
Every pull request that changes code, and every push to main, SHALL count
the AArch64 kernel's benchmarks in retired instructions under QEMU with
`-icount` (`cargo xtask bench --icount-only`) on a GitHub-hosted ARM64
runner, and SHALL fail when a gated count is worse than its baseline in
the `ci-arm` environment by more than the instructions threshold. The
results SHALL be published under annotation titles distinct from the
x86_64 job's.

#### Scenario: AArch64-only regression
- **WHEN** a pull request adds more than 2 % of instructions to a gated operation on AArch64 only
- **THEN** the AArch64 instruction-count CI job fails naming the `qemu-virt-icount` metric

#### Scenario: Same operations on both architectures
- **WHEN** the AArch64 kernel runs under `-icount` with performance counters in the guest
- **THEN** the results hold the fork benchmarks too, counted with the clock where no counter pass covers them

### Requirement: File metrics are gated
The `ci` baseline SHALL include `cell.file_write_read_us` and
`cell.fork_with_files_us` for every VMM that measures cells, so the
benchmark run on main fails when a change makes them slower beyond the
threshold.

#### Scenario: Slower file input and output
- **WHEN** a change makes `cell.file_write_read_us` slower than its `ci` baseline by more than the timing threshold
- **THEN** `cargo xtask bench --env ci --check` fails and names the metric

### Requirement: Server tail latencies are gated
The `ci` baseline SHALL include `density.sched_p99_us` and
`density.wake_p99_us` for every VMM that runs the server workload, so the
benchmark run on main fails when a change makes them slower beyond the
timing threshold.

#### Scenario: A longer wake-up tail
- **WHEN** a change makes `density.wake_p99_us` slower than its `ci` baseline by more than the timing threshold
- **THEN** `cargo xtask bench --env ci --check` fails and names the metric

## ADDED Requirements

### Requirement: Full benchmark gate on main
Every push to main, and a nightly scheduled run, SHALL run every benchmark
with the full gate in the `ci` environment: timings, and the metrics that
have no instruction count.

#### Scenario: Timing regression
- **WHEN** a change merged into main makes a timing metric worse than its `ci` threshold
- **THEN** the benchmark run on main fails naming the metric

### Requirement: Failing timing gate reported
When the benchmark gate fails on main or in the scheduled run, CI SHALL
open an issue, or comment on the open one, naming each failing metric, the
run, and the commits since the last successful benchmark run on main. Only
that reporting step SHALL have write access to issues.

#### Scenario: Regression on main
- **WHEN** the gate on main fails on `cell.fork_us/rhizo/firecracker`
- **THEN** the timing-gate issue names that metric, links the run, and lists the commits since the last green benchmark run
