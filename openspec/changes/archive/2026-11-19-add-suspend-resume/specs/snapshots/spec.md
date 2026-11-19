# Spec Delta

## Purpose

Defines suspending a cell into a snapshot, the snapshot format, and resuming
it, eagerly or lazily, with the guarantee that a resumed cell behaves as if
it had never stopped.

## ADDED Requirements

### Requirement: Suspend
The kernel SHALL suspend a live cell into a snapshot holding its registers,
program break, mapping cursor, remaining sleep and every mapped page with its
permissions and contents, and SHALL return all of the cell's memory and page
tables to the allocator.

#### Scenario: Memory returned
- **WHEN** a cell is suspended
- **THEN** the free memory is what it was before the cell was spawned, plus nothing held by the cell

### Requirement: Resume
The kernel SHALL resume a snapshot as a new live cell that continues from the
exact point where it was suspended.

#### Scenario: Equivalence
- **WHEN** a cell is suspended and resumed after every scheduling step until it exits
- **THEN** its complete output and exit status are identical to an uninterrupted run

### Requirement: Lazy resume
In lazy mode, the resumed cell SHALL become runnable before its pages are
restored; each page SHALL be restored the first time the cell or a system
call on its behalf touches it.

#### Scenario: Untouched pages
- **WHEN** a lazily resumed cell exits after touching only some of its pages
- **THEN** only those pages were restored, and its output is the same as with an eager resume

### Requirement: Snapshot format
A snapshot SHALL encode to a versioned byte sequence tagged with its
architecture, independent of in-memory layouts, and decoding SHALL reject a
sequence that is truncated, corrupted, of another version or of another
architecture with an error naming the reason.

#### Scenario: Wrong architecture
- **WHEN** a snapshot taken on one architecture is decoded on the other
- **THEN** decoding fails with an architecture error and no cell is created

#### Scenario: Deterministic encoding
- **WHEN** the same cell state is encoded twice
- **THEN** both byte sequences are identical
