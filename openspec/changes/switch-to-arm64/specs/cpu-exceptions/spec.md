# Spec Delta

## RENAMED Requirements

- FROM: `### Requirement: Double faults use a separate stack`
- TO: `### Requirement: Exceptions run on a separate stack`

## MODIFIED Requirements

### Requirement: Exceptions are reported, not silent
Every CPU exception raised in kernel mode SHALL be reported on the boot console
with its name, its architectural code (vector and error code on x86_64,
exception class and syndrome on AArch64) and the address of the faulting
instruction, and the VM SHALL then exit with the failure status.

#### Scenario: Invalid opcode
- **WHEN** kernel code executes an invalid instruction
- **THEN** the console shows an exception report naming the invalid-instruction exception with its code and instruction address
- **AND** the VM exits with the failure status

### Requirement: Page faults include fault details
A memory-access fault report (page fault on x86_64, data or instruction abort
on AArch64) SHALL also include the faulting virtual address and whether the
access was a read, a write or an instruction fetch, and whether the page was
not present (translation fault) or the access violated its protection.

#### Scenario: Write to an unmapped page
- **WHEN** kernel code writes to address `0x0`
- **THEN** the report names a fault at address `0x0` caused by a write to a page that is not present

### Requirement: Exceptions run on a separate stack
Exceptions caused by an exhausted kernel stack SHALL be handled on a dedicated
stack (the double-fault IST stack on x86_64, `SP_EL1` on AArch64 while the
kernel runs on `SP_EL0`), so that a stack overflow is still reported.

#### Scenario: Stack exhausted
- **WHEN** the kernel stack overflows into its guard page
- **THEN** the console shows an exception report for it and the VM exits with the failure status
