## MODIFIED Requirements

### Requirement: Snapshot format
A snapshot SHALL encode to a versioned byte sequence tagged with its
architecture, independent of in-memory layouts, and decoding SHALL reject a
sequence that is truncated, corrupted, of another version or of another
architecture with an error naming the reason. Version 8 SHALL record, for
each page, whether it is a write-or-execute page, and the cell's
write-or-execute ranges.

#### Scenario: Wrong architecture
- **WHEN** a snapshot taken on one architecture is decoded on the other
- **THEN** decoding fails with an architecture error and no cell is created

#### Scenario: Deterministic encoding
- **WHEN** the same cell state is encoded twice
- **THEN** both byte sequences are identical

#### Scenario: Write-or-execute page writable and executable
- **WHEN** a page record marked write-or-execute is both writable and executable, or neither
- **THEN** decoding fails with a page error

#### Scenario: Bad ranges
- **WHEN** a snapshot's write-or-execute ranges are unaligned, empty, out of order, touching, past `USER_END` or more than 64
- **THEN** decoding fails with a ranges error

## ADDED Requirements

### Requirement: Write-or-execute memory across snapshots
A resumed cell SHALL find its write-or-execute pages in the state they were
in, with their contents, and its write-or-execute ranges, whose untouched
pages SHALL still come zeroed on first touch, eagerly or lazily resumed.

#### Scenario: Code across a suspend
- **WHEN** a cell that wrote and ran code in write-or-execute memory is suspended, resumed, rewrites the code and runs it, and touches a new page of its range
- **THEN** the old code runs before the rewrite and the new one after, and the new page reads as zero
