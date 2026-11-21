## ADDED Requirements

### Requirement: Control path to first output
`cargo xtask bench` SHALL report, on every VMM where the control kernel
runs, the host-measured time from writing a request that starts a cell to
reading the first byte of that cell's output: `run` of `hello`
(`control.spawn_to_first_output_us`) and `fork` of a template frozen in
its sleep (`control.fork_to_first_output_us`), each the median of 20.

#### Scenario: Metrics present
- **WHEN** `cargo xtask bench` runs on QEMU `microvm`
- **THEN** the results contain `control.spawn_to_first_output_us` and `control.fork_to_first_output_us` for guest `rhizo` on `qemu-microvm`

### Requirement: Control path breakdown
With those metrics `cargo xtask bench` SHALL report, ungated, where the
time goes: each start's reply round trip, the kernel's time for it and the
output watch; the kernel's time for a `list`; the serial line's cost per
byte into and out of the VM; and a `list` round trip after 300 ms of
quiet (`breakdown.control.*`).

#### Scenario: Breakdown present
- **WHEN** `cargo xtask bench` runs on QEMU `microvm`
- **THEN** the results contain `breakdown.control.serial_in_ns_per_byte`, `breakdown.control.serial_out_ns_per_byte` and `breakdown.control.spawn_kernel_us`, none of them gated

### Requirement: Control plane measured alone
`cargo xtask bench --workload control` SHALL measure only the control
plane's figures, on the VMMs selected.

#### Scenario: Control alone
- **WHEN** `cargo xtask bench --workload control --vmm qemu-microvm` runs
- **THEN** its results hold the `control.` and `breakdown.control.` metrics of `qemu-microvm` and no boot or cell metric
