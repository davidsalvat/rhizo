# Spec Delta

## ADDED Requirements

### Requirement: Shared frames and tables
A frame or page table SHALL be returned to the allocator only when the last
cell referring to it releases it.

#### Scenario: Parent ends first
- **WHEN** a parent ends while its children still run
- **THEN** the children keep their memory, and after all end every frame is free again

### Requirement: Effective permissions
A page SHALL be writable only if its entry and every table entry above it
allow writing, for cell code and for kernel accesses on a cell's behalf.

#### Scenario: Read-only table
- **WHEN** a table entry is read-only and the page below it is writable
- **THEN** translation reports the page read-only
