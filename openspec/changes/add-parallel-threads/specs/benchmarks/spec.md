## ADDED Requirements

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
