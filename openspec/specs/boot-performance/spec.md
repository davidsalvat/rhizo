# boot-performance Specification

## Purpose
Defines which virtual machine monitors and machine types Rhizo boots on, the
fast boot console, the optimized build profile, and how boot time is measured
from the host's point of view.

## Requirements

### Requirement: Fast machine type by default
`cargo xtask run` and `cargo xtask test` SHALL boot the kernel on QEMU's
`virt` machine on AArch64 and on QEMU's `microvm` machine on x86_64 by default,
using hardware acceleration (KVM) when the host offers it for that
architecture. QEMU's `q35` machine SHALL remain selectable on x86_64.

#### Scenario: Default run
- **WHEN** a developer runs `cargo xtask run` on an AArch64 host with usable KVM
- **THEN** QEMU is started with the `virt` machine and KVM acceleration

#### Scenario: Selecting q35
- **WHEN** a developer runs `cargo xtask --arch x86_64 run --machine q35`
- **THEN** QEMU is started with the `q35` machine

### Requirement: Boots on Firecracker and Cloud Hypervisor
The same kernel image SHALL boot on Firecracker and Cloud Hypervisor without
VMM-specific builds (PVH on x86_64, the arm64 Image protocol on AArch64) and
reach the ready marker.

#### Scenario: Firecracker boot
- **WHEN** `cargo xtask run --vmm firecracker` is run on a host with KVM for that architecture
- **THEN** the kernel prints the banner and the ready marker and the VMM process ends

#### Scenario: Cloud Hypervisor boot
- **WHEN** `cargo xtask run --vmm cloud-hypervisor` is run on a host with KVM for that architecture
- **THEN** the kernel prints the banner and the ready marker and the VMM process ends

#### Scenario: No KVM
- **WHEN** Firecracker or Cloud Hypervisor is requested on a host without usable KVM for that architecture
- **THEN** `xtask` fails with a message saying KVM is required

### Requirement: Fast boot console
The kernel SHALL write console output to the fastest output the VMM offers: on
x86_64, the debug port `0xE9` when the VMM provides it, without per-byte
status polling, falling back to the first serial port; on AArch64, the UART
the device tree describes.

#### Scenario: Debug port available
- **WHEN** the x86_64 kernel runs on QEMU with the debug console enabled
- **THEN** all boot output arrives through the debug port

#### Scenario: No debug port
- **WHEN** the x86_64 kernel runs on Firecracker
- **THEN** all boot output arrives through the serial port

#### Scenario: AArch64
- **WHEN** the AArch64 kernel runs on QEMU `virt`
- **THEN** all boot output arrives through the PL011 UART

### Requirement: Optimized release profile
Release builds SHALL use link-time optimization, a single code-generation unit,
optimization level 3 and abort-on-panic. Benchmarks SHALL always run release
builds.

#### Scenario: Benchmark build
- **WHEN** `cargo xtask bench` builds the kernel
- **THEN** it builds the release profile with those settings

### Requirement: Host-measured boot time
For every boot it starts, `xtask` SHALL measure the wall-clock time from
launching the VMM process to receiving the ready marker, and report it together
with the in-kernel cycle count carried by the marker.

#### Scenario: Run report
- **WHEN** `cargo xtask run` completes a boot
- **THEN** `xtask` prints the time to ready in milliseconds and the in-kernel cycles

### Requirement: Large pages for the direct map
The direct map SHALL use 1 GiB pages when the CPU supports them (always on
AArch64 with a 4 KiB granule) and 2 MiB pages otherwise, without changing what
is mapped or its permissions.

#### Scenario: CPU with 1 GiB pages
- **WHEN** the CPU supports 1 GiB pages
- **THEN** the direct map of a RAM gigabyte is a single 1 GiB mapping, readable, writable and not executable
