# Spec Delta

## ADDED Requirements

### Requirement: Suspended cells use no memory but their snapshot
A suspended cell SHALL hold no frames, page tables or address-space tag; its
whole state SHALL be its snapshot.

#### Scenario: Many suspended cells
- **WHEN** 32 cells are suspended
- **THEN** no cell is live (no page table, frame or tag is held) and each snapshot can be resumed and run to completion
