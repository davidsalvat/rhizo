# Spec Delta

## ADDED Requirements

### Requirement: numpy as a Linux process in the same VM
`cargo xtask bench` SHALL measure, where the packages are built and
Firecracker can run, the same CPython, numpy and script as Linux processes
inside a Firecracker guest: started cold with `fork` and `execve`, and
forked from a CPython process that imported numpy, each until the process
that computed has exited.

#### Scenario: Measured in the guest
- **WHEN** `cargo xtask bench` runs on a host with KVM for the target architecture and the packages built
- **THEN** the results contain `workload.numpy_cold_us` and `workload.numpy_warm_us` for guest `linux-process` on `firecracker`

#### Scenario: Not available
- **WHEN** the packages are not built or Firecracker cannot run on the host
- **THEN** the comparison is listed as skipped with the reason, and no number is reported
