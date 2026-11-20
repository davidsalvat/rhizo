# Spec Delta

## ADDED Requirements

### Requirement: The CPU refuses kernel accesses to cell pages
On CPUs that have them, the kernel SHALL run with SMEP and SMAP (x86_64)
or PAN (AArch64) on, so a kernel load, store or instruction fetch at a
cell's address faults. The kernel SHALL reach cell memory only through
the user-memory copies of `rhizo_cell::memory`, which use the direct map.

#### Scenario: Stray kernel read of a user page
- **WHEN** kernel code reads a page mapped for a cell, by its user address
- **THEN** the CPU raises a permission fault in kernel mode instead of returning the data

#### Scenario: Stray kernel jump into a user page
- **WHEN** kernel code calls a page mapped for a cell
- **THEN** the CPU raises a permission fault on the instruction fetch

### Requirement: A cell's flags never reach kernel code
Flags a cell sets for itself that change how the kernel's code runs
(x86_64 `RFLAGS.DF` and `RFLAGS.AC`) SHALL be cleared on every entry to
the kernel from the cell, by system call, exception or interrupt, and
SHALL be the cell's again when it resumes.

#### Scenario: Interrupted with the direction flag set
- **WHEN** a cell spins with the direction and alignment-check flags set and is interrupted many times, next to other cells
- **THEN** the kernel runs with both clear after every interruption, the cell finds its direction flag as it set it, and the other cells finish with their expected output
