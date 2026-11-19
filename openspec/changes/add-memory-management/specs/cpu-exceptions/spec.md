# Spec Delta

## Purpose

Defines how CPU exceptions raised while the kernel runs are reported and
handled.

## ADDED Requirements

### Requirement: Exceptions are reported, not silent
Every CPU exception (vectors 0 to 31) raised in kernel mode SHALL be reported on
the serial console with its name, vector number, error code and the address of
the faulting instruction, and the VM SHALL then exit with the failure status.

#### Scenario: Invalid opcode
- **WHEN** kernel code executes an invalid instruction
- **THEN** the console shows an exception report naming the invalid opcode exception with its vector and instruction address
- **AND** the VM exits with the failure status

### Requirement: Page faults include fault details
A page fault report SHALL also include the faulting virtual address and whether
the access was a read, a write or an instruction fetch, and whether the page
was not present or the access violated its protection.

#### Scenario: Write to an unmapped page
- **WHEN** kernel code writes to address `0x0`
- **THEN** the report names a page fault at address `0x0` caused by a write to a page that is not present

### Requirement: Double faults use a separate stack
The double-fault handler SHALL run on a dedicated stack, so that a fault caused
by an exhausted kernel stack is still reported.

#### Scenario: Stack exhausted
- **WHEN** the kernel stack overflows into its guard page
- **THEN** the console shows a double fault report and the VM exits with the failure status
