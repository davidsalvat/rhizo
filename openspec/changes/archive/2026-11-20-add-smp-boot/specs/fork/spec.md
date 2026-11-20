# Spec Delta

## ADDED Requirements

### Requirement: Copy-on-write without a free large frame
A write to a 2 MiB page shared copy-on-write SHALL succeed whenever memory
holds 2 MiB of free frames, contiguous or not: with no free 2 MiB block,
the writer's copy SHALL be made of small pages.

#### Scenario: Fragmented memory
- **WHEN** a cell writes to a large page it shares with its parent and no 2 MiB block is free
- **THEN** the write succeeds in a private copy made of small pages, the parent's page is unchanged, and every frame is returned when both end
