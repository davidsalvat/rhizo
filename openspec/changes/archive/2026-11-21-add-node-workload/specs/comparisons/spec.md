# Spec Delta

## ADDED Requirements

### Requirement: Node.js as a Linux process in the same VM
`cargo xtask bench` SHALL measure, where Node.js is packed and Firecracker
can run, the same Node.js files and request as a Linux process inside a
Firecracker guest: started cold with `fork` and `execve` until it exits.
Node.js cannot fork itself, so no warm figure is reported for Linux.

#### Scenario: Measured in the guest
- **WHEN** `cargo xtask bench` runs on a host with KVM for the target architecture and `RHIZO_NODE` set
- **THEN** the results contain `workload.node_cold_us` for guest `linux-process` on `firecracker`

#### Scenario: Not available
- **WHEN** Node.js is not packed or Firecracker cannot run on the host
- **THEN** the comparison is listed as skipped with the reason, and no number is reported
