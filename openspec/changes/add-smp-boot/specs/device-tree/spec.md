# Spec Delta

## ADDED Requirements

### Requirement: CPUs from the device tree
The kernel SHALL read its CPUs from the children of `/cpus` whose
`device_type` is `cpu`: the `reg` value (with the `#address-cells` of
`/cpus`) as the CPU's affinity, whether `enable-method` lists `psci`, and
SHALL skip CPUs whose `status` is neither `okay` nor `ok`.

#### Scenario: QEMU virt with four CPUs
- **WHEN** the device tree of `-M virt -smp 4` is read
- **THEN** it lists four CPUs, affinities 0 to 3, each started through PSCI

#### Scenario: Two-cell identifiers and disabled CPUs
- **WHEN** `/cpus` has `#address-cells = <2>`, a disabled CPU and a node that is not a CPU
- **THEN** identifiers are read as 64-bit values, and only the available CPUs are listed
