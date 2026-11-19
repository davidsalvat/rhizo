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
