# comparisons Specification

## Purpose
Defines how Rhizo is compared with existing isolation mechanisms: measured
on the same machine, in the same run, with the same timing method.

## Requirements

### Requirement: Firecracker snapshot restore
`cargo xtask bench` SHALL measure, where Firecracker can run, the time from
launching a new Firecracker process to a restored Linux guest running, from
a full snapshot taken through Firecracker's API.

#### Scenario: Restore measured
- **WHEN** `cargo xtask bench` runs on an x86_64 host with KVM
- **THEN** the results contain `snapshot.restore_ms` and `snapshot.restore_api_ms` for guest `linux` on `firecracker`

#### Scenario: Not available
- **WHEN** Firecracker cannot run on the host
- **THEN** the comparison is listed as skipped with the reason, and no number is reported

### Requirement: Linux process baselines
`cargo xtask bench` SHALL measure, on a Linux host of the target
architecture, from a process holding 50 MiB of written memory: `fork`
until the child exits and is waited for, the parent's `fork()` call alone,
and a fork whose child writes every page; and spawning the same static
program the cell benchmark runs.

#### Scenario: Process metrics
- **WHEN** `cargo xtask bench` runs on such a host
- **THEN** the results contain `process.fork_us`, `process.fork_call_us`, `process.fork_touch_us` and `process.spawn_us`

### Requirement: gVisor sandboxes
`cargo xtask bench` SHALL measure, where gVisor's `runsc` is installed and
runs, a new gVisor sandbox running the cell benchmark's static `hello` to
its exit, and the restore of a checkpointed gVisor sandbox until it runs.

#### Scenario: gVisor measured
- **WHEN** `cargo xtask bench` runs on a host with `runsc` and root access
- **THEN** the results contain `sandbox.run_ms` and `snapshot.restore_ms` for guest `gvisor`

#### Scenario: gVisor not available
- **WHEN** `runsc` is missing or cannot run
- **THEN** the gVisor comparison is listed as skipped with the reason, and no number is reported

### Requirement: Linux processes inside the same VM
`cargo xtask bench` SHALL measure, where Firecracker can run, Linux
processes inside a Firecracker guest: the parent's `fork()` call and a fork
whose child writes all 50 MiB, from a process holding 50 MiB written, and
`posix_spawn` of the same static program the cell benchmark runs.

#### Scenario: Measured in the guest
- **WHEN** `cargo xtask bench` runs on a host with KVM for the target architecture
- **THEN** the results contain `process.fork_call_us`, `process.fork_touch_us` and `process.spawn_us` for guest `linux-process` on `firecracker`

#### Scenario: Not available
- **WHEN** Firecracker cannot run on the host
- **THEN** the comparison is listed as skipped with the reason, and no number is reported

### Requirement: CRIU process restore
`cargo xtask bench` SHALL measure, where CRIU is installed and runs, the
restore of a dumped Linux process holding as much written memory as the
cell snapshot, until it runs again.

#### Scenario: CRIU measured
- **WHEN** `cargo xtask bench` runs on a host with CRIU and root access
- **THEN** the results contain `snapshot.restore_ms` for guest `criu` on `host`

#### Scenario: CRIU not available
- **WHEN** CRIU is missing or cannot run
- **THEN** the CRIU comparison is listed as skipped with the reason, and no number is reported

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

### Requirement: The agent as a Linux process in the same VM
`cargo xtask bench` SHALL run the CPython agent as a Linux process inside
the Firecracker guest of the CPython comparison, reaching the host's fake
model through the guest's network card and forking its branches with
`fork`, and report its turn and its fork until every branch runs; where it
cannot, it SHALL list them as skipped with the reason.

#### Scenario: Measured in the guest
- **WHEN** `cargo xtask bench` runs on a host with KVM, password-less `sudo` and CPython built
- **THEN** the results contain `workload.agent_cpython_turn_us` and `workload.agent_fork_us` for guest `linux-process` on `firecracker`

#### Scenario: No tap device
- **WHEN** the tap device cannot be made
- **THEN** the agent comparison is listed as skipped with the reason, and the CPython figures are still reported

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
