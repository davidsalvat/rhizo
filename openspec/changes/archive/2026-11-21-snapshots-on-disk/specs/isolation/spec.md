# Spec Delta

## MODIFIED Requirements

### Requirement: Snapshots from outside are contained
Snapshot bytes SHALL be treated as written by anyone: any bytes SHALL be
refused with a format error, or resume into a cell that changes no kernel
memory or kernel page-table entry, maps only user pages below `USER_END`
and never writable and executable at once, keeps only write-or-execute
ranges below `USER_END` and at most 64 of them, starts every thread below
`USER_END`, has files whose every operation answers instead of panicking,
and gives every frame back when it ends.

#### Scenario: Tampered or random snapshot
- **WHEN** a valid snapshot with bytes changed or cut, or random bytes behind a valid header, are read and resumed, with their checksum made right again or not
- **THEN** they are refused, or the resumed cell holds the properties above

## ADDED Requirements

### Requirement: Loaded snapshots claim nothing more
Bytes the host loads SHALL go through that validation, checksum included.
No page of them SHALL refer to a physical frame, and the cell they make
SHALL have no higher quotas than a cell the host starts.

#### Scenario: A page that claims a frame
- **WHEN** an exported snapshot's page record is marked as referring to a frame, its checksum made right
- **THEN** decoding refuses it

#### Scenario: Quotas from a file
- **WHEN** a decoded snapshot's quotas are capped by those of a new cell, or by lower ones
- **THEN** each quota is the lower of the two, and the CPU time used is kept
