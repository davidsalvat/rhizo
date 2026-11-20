# Spec Delta

## ADDED Requirements

### Requirement: CPython as a Linux process in the same VM
`cargo xtask bench` SHALL measure, where CPython is built and Firecracker
can run, the same CPython, standard library and request as Linux processes
inside a Firecracker guest: started cold with `fork` and `execve`, and
forked from a CPython process that imported and warmed up the script, each
until the process that answered has exited.

#### Scenario: Measured in the guest
- **WHEN** `cargo xtask bench` runs on a host with KVM for the target architecture and CPython built
- **THEN** the results contain `workload.cpython_cold_us` and `workload.cpython_warm_us` for guest `linux-process` on `firecracker`

#### Scenario: Not available
- **WHEN** CPython is not built or Firecracker cannot run on the host
- **THEN** the comparison is listed as skipped with the reason, and no number is reported
