# Spec Delta

## MODIFIED Requirements

### Requirement: Unsupported calls are visible
Any other system call SHALL return `-ENOSYS` and the kernel SHALL print the
call's name or number and the cell once, so missing calls are found quickly.
Numbers from 1023 up, past every Linux call, SHALL share one report, so a
cell cannot make the kernel print without end.

#### Scenario: Unknown call
- **WHEN** a cell invokes a system call Rhizo does not implement
- **THEN** it receives `-ENOSYS` and the console shows `unimplemented syscall` with the call

#### Scenario: Endless unknown numbers
- **WHEN** a cell invokes many different call numbers past every Linux one
- **THEN** each receives `-ENOSYS` and the console shows one report for all of them
