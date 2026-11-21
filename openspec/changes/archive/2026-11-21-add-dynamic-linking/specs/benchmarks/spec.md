# Spec Delta

## ADDED Requirements

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
