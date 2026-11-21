## ADDED Requirements

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
