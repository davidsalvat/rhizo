## MODIFIED Requirements

### Requirement: File pages shared between cells
The kernel SHALL keep each file page it maps into a cell in one frame, by
the identity of its contents, and SHALL map that frame into every mapping
of the same bytes, in any cell, read-only; a write SHALL go to a private
copy. A page whose file changed SHALL NOT be mapped for the new contents.
Kept pages SHALL belong to the kernel, be taken like a cell's memory
(never from its reserve) and, if unmapped or held by no file, be dropped
with the kept executables before memory is refused to any cell.

#### Scenario: Two cells of one program
- **WHEN** two cells of the same dynamically linked program are started from the same files
- **THEN** their interpreter's code is the same frame in both, and each sees only its own writes to its data

#### Scenario: A file written after it was mapped
- **WHEN** a cell writes a file whose pages are kept and then maps it
- **THEN** the new mapping holds the new bytes and the earlier mappings the old ones

#### Scenario: Kept pages give way
- **WHEN** dynamically linked cells have ended and the kept executables and pages are dropped
- **THEN** free memory and the kernel heap are what they were before the cells started

## ADDED Requirements

### Requirement: No cell evicts the pages another maps
No cell SHALL make the kernel drop a kept file page that a mapping uses
and a file holds. When the kept pages reach their limit, the kernel SHALL
drop only pages no file holds any more, and a page it has no room for
SHALL be a copy of the mapping cell's own, counted in its memory quota.

#### Scenario: A full cache
- **WHEN** one cell maps a kept page and another maps more pages of its own files than the kept pages' limit
- **THEN** the first cell's page stays kept, and the pages past the limit are the second cell's own copies

#### Scenario: Memory pressure from another cell
- **WHEN** a cell maps a page of its file, and another cell takes memory until it is refused
- **THEN** the first cell's page stays kept
