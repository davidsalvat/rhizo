# Spec Delta

## ADDED Requirements

### Requirement: In-kernel tests on several CPUs
`cargo xtask test` SHALL also run the whole in-kernel test suite on a VM
with 4 vCPUs (step `ktest-smp`), passing the CPU count each VMM takes
(QEMU `-smp`, Firecracker `vcpu_count`, Cloud Hypervisor `--cpus boot=`),
and SHALL fail unless the kernel reports `cpus: 4 online` and every test
passes. CI SHALL run it for both architectures on QEMU, and on Firecracker
and Cloud Hypervisor where KVM is available.

#### Scenario: Rerun on several CPUs
- **WHEN** a developer runs `cargo xtask test --only ktest-smp`
- **THEN** the in-kernel tests run on 4 vCPUs and the step reports their result
