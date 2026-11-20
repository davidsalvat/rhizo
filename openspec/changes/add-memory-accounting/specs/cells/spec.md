# Spec Delta

## ADDED Requirements

### Requirement: Memory pressure is the cell's error
When cells together exhaust memory, the kernel SHALL keep a reserve for
itself and SHALL report the shortage to the cell that asked (`ENOMEM`,
`ENOSPC`, or a fault on a page it cannot get) or to the host (`spawn`,
`fork`, `suspend` or eager `resume` fails and changes nothing). The kernel
SHALL NOT panic, and SHALL get every frame back when the cells end.

#### Scenario: Cells fill memory
- **WHEN** more cells than memory can hold each map and write 8 MiB, or write 16 MiB of files
- **THEN** some are refused, the others finish, the kernel keeps running, and once all have ended free memory is what it was before

### Requirement: Thousands of idle cells
The kernel SHALL hold at least 1000 live idle cells forked from one
template at once, each costing a few pages beyond what it shares.

#### Scenario: 1000 idle cells
- **WHEN** a template of an idle cell is forked 1000 times and every child runs into its sleep
- **THEN** all 1000 are live, each costs less than 64 KiB of memory, and killing them returns all of it
