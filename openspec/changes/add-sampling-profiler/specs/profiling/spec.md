## ADDED Requirements

### Requirement: Sampling where cell code runs
A kernel built with the `sampling` feature SHALL be able to sample, on every
CPU, which cell runs and at which address: with the PMU
counting cycles of cell code only and interrupting every 100,000 of them
where the guest has a PMU and its interrupt, else with the timer ticking
every 100 µs while a time slice still lasts 10 ms.

#### Scenario: A spinning cell
- **WHEN** a cell spins while sampling runs
- **THEN** there are samples, each of that cell, at an address of the lower half

### Requirement: Samples folded in the kernel
The kernel SHALL keep samples in a buffer whose room is reserved before
sampling starts, count the samples a full buffer loses, and print them
folded: one line per cell and address with its count.

#### Scenario: Repeated addresses
- **WHEN** the same cell is sampled three times at one address
- **THEN** one line carries that cell, that address and the count 3

### Requirement: Folded stacks per workload and system call
`cargo xtask profile` SHALL run the cell loop, `fork_touch` and the CPython
cold start (where built) sampled and accounted, name each address with the
symbols of the image (an address without one folds by 4 KiB page), and
write per workload folded stacks in nanoseconds: the cells' functions
sharing the accounted user time by sample count, beside the kernel's parts,
and the system calls that took most time.

#### Scenario: CPython cold start
- **WHEN** `cargo xtask profile` runs where CPython is built
- **THEN** `cpython_cold.folded` holds lines `cpython_cold;cpython;<function> <ns>` and `cpython_cold;kernel;<part> <ns>`, and `cpython_cold.calls.folded` lines `cpython_cold;<call> <ns>`
