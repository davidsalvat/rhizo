## ADDED Requirements

### Requirement: Cell bookkeeping does not grow with the cells
The kernel's record of a cell's place in the ownership tree SHALL cost a
cell that ends, and a cell that is resumed, an amortized constant amount
of work, whatever the number of other cells, so that the cells still
running do not wait behind the ends and resumes of others.

#### Scenario: Thousands of cells end together
- **WHEN** 10,000 cells of the ownership tree end one after another
- **THEN** the tree's work for all of them together is at most a constant number of passes over the tree, not one per cell, and every remaining cell keeps its node, place, domain and number

#### Scenario: Resumes among thousands of cells
- **WHEN** a cell suspended among 10,000 others is resumed under a new identifier
- **THEN** it keeps its place, parent, children and domain, takes a new number, and no other cell's node moves

#### Scenario: The server workload's latencies
- **WHEN** the server workload of `cargo xtask bench` runs 10,000 workers on QEMU `microvm`
- **THEN** `density.sched_p99_us` and `density.wake_p99_us` are within the `ci` thresholds of the baseline set before the ownership tree, and lazy resumes (`resume_ns` in its breakdown line) cost what they did then
