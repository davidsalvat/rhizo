## ADDED Requirements

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
