# Spec Delta

## ADDED Requirements

### Requirement: Suspended cells use no memory but their snapshot
A suspended cell SHALL hold no frames, page tables or address-space tag; its
whole state SHALL be its snapshot.

#### Scenario: Many suspended cells
- **WHEN** more cells are suspended than could be live at once with their memory
- **THEN** each can still be resumed and run to completion
