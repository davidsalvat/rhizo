# Spec Delta

## ADDED Requirements

### Requirement: Cell faults end the cell, not the kernel
An exception raised by cell code SHALL be reported with the same details as a
kernel exception, prefixed by the cell identifier, and SHALL end only that
cell; the kernel SHALL continue.

#### Scenario: Null write in a cell
- **WHEN** a cell writes to address `0x0`
- **THEN** the console shows `cell <id> killed:` followed by a fault report for a write at `0x0`, and the kernel continues with its next task
