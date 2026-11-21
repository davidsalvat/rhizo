## MODIFIED Requirements

### Requirement: Suspend
The kernel SHALL suspend a live cell into a snapshot holding, for each of
its threads, its registers, FP/SIMD state, thread pointer, identifier,
clear-tid address and what it waits for (a sleep or a futex, with the time
left), and the cell's program break, mapping cursor and every mapped page
with its permissions and contents, and SHALL return all of the cell's
memory and page tables to the allocator, except the frames the snapshot
refers to (requirement "Shared pages in snapshots").

#### Scenario: Memory returned
- **WHEN** a cell is suspended and its snapshot is dropped
- **THEN** the free memory is what it was before the cell was spawned, plus nothing held by the cell

#### Scenario: Waiting threads in a snapshot
- **WHEN** a cell with a thread blocked on a futex is suspended and resumed, and another thread then wakes the futex
- **THEN** the blocked thread wakes and the program completes as without suspension

## ADDED Requirements

### Requirement: Shared pages in snapshots
A snapshot taken by the kernel SHALL refer to each page its cell shares
with another holder (a template, a fork, a loaded image) instead of
copying it, holding a reference to its frame until the snapshot is
dropped, and no holder SHALL be able to change a frame a snapshot refers
to. Resuming SHALL map a referred frame again, copy-on-write if the page
is writable.

#### Scenario: Suspended fork of a template
- **WHEN** a cell forked from a template that maps 2 MiB is suspended before it writes
- **THEN** its snapshot holds less than 64 KiB in the kernel heap and refers to the template's 2 MiB

#### Scenario: Template writes after a suspend
- **WHEN** a cell forked from a template is suspended, and the template's pages it referred to are written afterwards
- **THEN** the resumed cell finds its pages as they were when it was suspended, and the template finds its own writes

### Requirement: Exported snapshots stand alone
A snapshot exported to bytes SHALL hold the contents of the frames it
refers to in their place, in the snapshot format of the same version,
and decoding SHALL reject bytes that refer to a frame.

#### Scenario: Export stands alone
- **WHEN** a snapshot that refers to shared frames is exported and decoded again
- **THEN** the decoded snapshot refers to no frame and resumes to the same memory

#### Scenario: Reference from outside
- **WHEN** bytes whose page record is marked as referring to a frame are decoded
- **THEN** decoding fails with a page error
