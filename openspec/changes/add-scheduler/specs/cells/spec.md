# Spec Delta

## ADDED Requirements

### Requirement: Isolation between cells
Each cell SHALL have its own address space: the same virtual address in two
cells refers to different memory, and a cell SHALL NOT be able to read or
write memory mapped only in another cell. A fault in one cell SHALL NOT
affect any other cell.

#### Scenario: Same address, different memory
- **WHEN** two cells write different values at the same virtual address, yield to each other, and read it back
- **THEN** each reads its own value

#### Scenario: Another cell's mapping
- **WHEN** a cell touches an address that only another cell has mapped
- **THEN** it is killed with a fault report and the other cell is unaffected

### Requirement: Tagged address spaces
Switching between cells SHALL NOT flush translations of other cells when the
CPU supports address-space identifiers; a cell's translations SHALL be
removed when it ends, before its identifier is reused.

#### Scenario: Reused identifier
- **WHEN** more cells are created and ended than there are identifiers in use at once
- **THEN** every cell still sees only its own memory
