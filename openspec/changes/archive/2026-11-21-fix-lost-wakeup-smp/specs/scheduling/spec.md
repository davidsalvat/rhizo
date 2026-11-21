# Spec Delta

## ADDED Requirements

### Requirement: A run with a time limit ends on time
A host run given a time limit SHALL end by that limit, within a slice, on
any number of CPUs, whatever its cells wait for and whichever CPU's timer
holds their deadlines.

#### Scenario: Cells sleeping an hour
- **WHEN** one cell per CPU sleeps an hour on 1 or 4 vCPUs, and the host runs them 20 times with a limit of 50 ms each
- **THEN** each run ends within 500 ms (its limit and a slice, with room for an emulated machine), and the cells still sleep
