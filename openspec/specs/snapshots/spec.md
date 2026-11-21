# snapshots Specification

## Purpose
Defines suspending a cell into a snapshot, the snapshot format, and resuming
it, eagerly or lazily, with the guarantee that a resumed cell behaves as if
it had never stopped.

## Requirements

### Requirement: Suspend
The kernel SHALL suspend a live cell into a snapshot holding, for each of
its threads, its registers, FP/SIMD state, thread pointer, identifier,
clear-tid address and what it waits for (a sleep or a futex, with the time
left), and the cell's program break, mapping cursor and every mapped page
with its permissions and contents, and SHALL return all of the cell's
memory and page tables to the allocator.

#### Scenario: Memory returned
- **WHEN** a cell is suspended
- **THEN** the free memory is what it was before the cell was spawned, plus nothing held by the cell

#### Scenario: Waiting threads in a snapshot
- **WHEN** a cell with a thread blocked on a futex is suspended and resumed, and another thread then wakes the futex
- **THEN** the blocked thread wakes and the program completes as without suspension

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

### Requirement: Files in snapshots
A snapshot SHALL contain the cell's file system, open descriptors and
working directory, and a resumed cell SHALL find them as they were.

#### Scenario: Open file across suspend
- **WHEN** a cell with a file open is suspended and resumed between two reads
- **THEN** the second read continues at the same offset with the same data

### Requirement: Threaded cells across snapshots
Suspending and resuming a multi-threaded cell SHALL be invisible to it:
each thread continues where it was, in the same scheduling order.

#### Scenario: Threads after every step
- **WHEN** a multi-threaded glibc program using mutexes and condition variables is suspended and resumed after every scheduling step until it exits
- **THEN** its complete output and exit status are identical to an uninterrupted run

### Requirement: Pipes and sockets in snapshots
A snapshot (format version 7) SHALL contain the cell's pipes with their
data (the pipe of the host's standard input among them), its event
counters, its `epoll` instances and one record per socket
(family, type, state, address, options; for a parked listener or
connection, the boot identity of the kernel that parked it and its key
there); resuming SHALL
restore the first three as they were and the sockets as the `networking`
capability says. A record whose fields are out of range SHALL make the
snapshot invalid.

#### Scenario: Pipe across suspend
- **WHEN** a cell writes into a pipe, is suspended and resumed, and reads the pipe
- **THEN** it reads the bytes written before the suspend

#### Scenario: Blocked cell across suspend
- **WHEN** a cell blocked in a system call is suspended and resumed
- **THEN** it waits in the same call again, and completes it once a descriptor is ready

#### Scenario: Parked record round trip
- **WHEN** socket records, a parked one among them, are encoded and decoded
- **THEN** the records are equal, and a truncated or out-of-range record is refused
