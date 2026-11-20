# Spec Delta

## ADDED Requirements

### Requirement: Network card metric
Where the VMM gives the kernel a network card and a host echo server,
`cargo xtask bench` SHALL report `net.tcp_rtt_us`, the round-trip time of a
64-byte message between a cell and a TCP echo server on the host; elsewhere
it SHALL report the metric as skipped with the reason.

#### Scenario: Metric present
- **WHEN** `cargo xtask bench` runs on QEMU `microvm` with KVM
- **THEN** the results contain `net.tcp_rtt_us/rhizo/qemu-microvm`
