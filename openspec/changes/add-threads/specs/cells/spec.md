# Spec Delta

## MODIFIED Requirements

### Requirement: Cell state is plain data
Everything needed to resume a cell (each thread's registers, identifier,
clear-tid address and what it waits for; the address-space root, program
break and mapping cursor) SHALL be held in plain data with no pointers into
kernel memory, so it can be copied and serialized.

#### Scenario: Copying registers
- **WHEN** a cell's register state is copied bit for bit
- **THEN** the copy describes the same execution point

#### Scenario: Copying threads
- **WHEN** the thread table of a cell with blocked and runnable threads is copied
- **THEN** the copy has the same threads, in the same order, waiting for the same things

### Requirement: Floating-point state is per cell
Each thread of each cell SHALL have its own floating-point and SIMD
registers, kept across preemption, system calls, thread switches,
suspend/resume and fork, and never visible to another thread or cell.

#### Scenario: Two FP cells
- **WHEN** two cells hold different values in FP/SIMD registers across yields
- **THEN** each finds its own values

#### Scenario: FP across a snapshot
- **WHEN** a cell computing with FP/SIMD registers is suspended and resumed repeatedly
- **THEN** its result is the same as without suspension

#### Scenario: Two FP threads
- **WHEN** two threads of one cell hold different values in FP/SIMD registers while they take turns
- **THEN** each finds its own values
