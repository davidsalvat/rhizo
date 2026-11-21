# Spec Delta

## MODIFIED Requirements

### Requirement: Snapshot format
A snapshot SHALL encode to a versioned byte sequence (format version 10)
tagged with its architecture, independent of in-memory layouts, and decoding SHALL reject a
sequence that is truncated, corrupted, of another version or of another
architecture with an error naming the reason.

#### Scenario: Wrong architecture
- **WHEN** a snapshot taken on one architecture is decoded on the other
- **THEN** decoding fails with an architecture error and no cell is created

#### Scenario: Deterministic encoding
- **WHEN** the same cell state is encoded twice
- **THEN** both byte sequences are identical

## ADDED Requirements

### Requirement: Waits on descriptors across suspend
A snapshot SHALL record a thread waiting on descriptors (`poll`, `select`,
`epoll_wait`) as waiting, with the time its call had left, and a runnable
thread about to make such a call again with that call's time left. A
resumed cell's threads SHALL make their calls again with the time they had
left, not with their whole timeout.

#### Scenario: Time left across suspend
- **WHEN** a cell 1.5 s into a 2 s wait in `epoll_pwait` is suspended, its snapshot exported and decoded, and the cell resumed
- **THEN** the snapshot holds at most 0.5 s left for that call, and the resumed cell's call times out less than 1.5 s after the resume

#### Scenario: Waits decoded
- **WHEN** a snapshot holds a thread waiting on descriptors with a deadline, one waiting without, and a runnable one with a call's deadline kept, and it is decoded at a later time
- **THEN** the first two wait on descriptors again (the first with its time left from then), and the deadlines of the calls to make again are the first's and the third's, moved by the same time

### Requirement: Snapshot files
The exported form of a snapshot (the bytes that leave the kernel, as a
file) SHALL stand alone and end with the CRC-32C of everything before it.
Decoding SHALL check, in this order, its header (magic, version,
architecture), its checksum and then everything it holds, and SHALL refuse
a damaged or cut file with a checksum error. Page sizes are recorded per
page (4 KiB or 2 MiB, the same on both architectures); a kernel with
another format version SHALL refuse the file by its version.

#### Scenario: Damaged or cut file
- **WHEN** one byte after the header of an exported snapshot is changed, or its last byte is cut
- **THEN** decoding fails with a checksum error

#### Scenario: Errors in order
- **WHEN** an exported snapshot of another version, or of another architecture, is decoded
- **THEN** decoding fails with the version or architecture error, not a checksum error

### Requirement: Templates as files
The snapshot of a template SHALL hold its threads, memory, files and
sockets (none parked), and loaded as a template SHALL give a template:
never run, its copies starting where it was frozen, with no time kept, as
for any template.

#### Scenario: A template through bytes
- **WHEN** a cell waiting an hour in `epoll_pwait` is frozen into a template, its snapshot exported, decoded and loaded as a template, the first template dropped and two cells made from the loaded one
- **THEN** both cells time out at once and exit with 0, and every frame comes back once the loaded template is dropped
