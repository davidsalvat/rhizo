# Spec Delta

## ADDED Requirements

### Requirement: The same time on every CPU
Every CPU's timer SHALL count time at the boot CPU's rate: a time slice and
a timed wait of the kernel SHALL last as long on any CPU as on the boot
CPU, as measured by the counter.

#### Scenario: A timed wait on each CPU
- **WHEN** each CPU in turn waits 20 ms on its own timer, on 4 vCPUs
- **THEN** each wait lasts at least 18 ms by the counter
