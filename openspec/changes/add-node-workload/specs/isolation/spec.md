# Spec Delta

## ADDED Requirements

### Requirement: Counters a cell may read
A cell SHALL be able to read the CPU's counter directly, without a system
call, on every CPU it runs on: the virtual counter and its frequency
(`CNTVCT_EL0`, `CNTFRQ_EL0`) on AArch64 and the time-stamp counter
(`rdtsc`) on x86_64, as on Linux. The physical counter and the timers on
AArch64 SHALL stay the kernel's. The counter is not cell state: it keeps
running while a cell is suspended, as `CLOCK_MONOTONIC` does, and a
resumed or forked cell reads the machine's counter.

#### Scenario: Reading the counter
- **WHEN** a cell reads the counter twice
- **THEN** both readings are nonzero, the second is not smaller, and the cell keeps running
