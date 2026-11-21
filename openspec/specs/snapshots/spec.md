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
left), and the cell's program break, mapping cursor, quotas and CPU time
used, and every mapped page with its permissions and contents.

#### Scenario: Waiting threads in a snapshot
- **WHEN** a cell with a thread blocked on a futex is suspended and resumed, and another thread then wakes the futex
- **THEN** the blocked thread wakes and the program completes as without suspension

#### Scenario: Quotas in a snapshot
- **WHEN** a cell with quotas that used some CPU time is suspended and its snapshot decoded
- **THEN** the restored cell has the same quotas and the same CPU time used

### Requirement: Memory returned on suspend
Suspending a cell SHALL return all of its memory and page tables to the
allocator, except the frames the snapshot refers to (requirement "Shared
pages in snapshots").

#### Scenario: Memory returned
- **WHEN** a cell is suspended and its snapshot is dropped
- **THEN** the free memory is what it was before the cell was spawned, plus nothing held by the cell

### Requirement: Resume
The kernel SHALL resume a snapshot as a new live cell that continues from the
exact point where it was suspended.

#### Scenario: Equivalence
- **WHEN** a cell that ends every step of its work with `sched_yield`, and sleeps in some of them, is suspended and resumed after every scheduling step until it exits
- **THEN** its complete output and exit status are identical to an uninterrupted run, and it was suspended at least once per step

### Requirement: Lazy resume
In lazy mode, the resumed cell SHALL become runnable before its pages are
restored; each page SHALL be restored the first time the cell or a system
call on its behalf touches it.

#### Scenario: Untouched pages
- **WHEN** a lazily resumed cell exits after touching only some of its pages
- **THEN** only those pages were restored, and its output is the same as with an eager resume

### Requirement: Snapshot format
A snapshot SHALL encode to a versioned byte sequence (format version 10)
tagged with its architecture, independent of in-memory layouts, and decoding SHALL reject a
sequence that is truncated, corrupted, of another version or of another
architecture with an error naming the reason. It SHALL record, for each
page, whether it is a write-or-execute page, and the cell's
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
A snapshot (since format version 7) SHALL contain the cell's pipes with their
data (the pipe of the host's standard input among them), its event
counters, its `epoll` instances and one record per socket
(family, type, state, address, options; for a parked listener or
connection, the boot identity of the kernel that parked it and its key
there); resuming SHALL
restore the first three as they were and the sockets as the `networking`
capability says.

#### Scenario: Pipe across suspend
- **WHEN** a cell writes into a pipe, is suspended and resumed, and reads the pipe
- **THEN** it reads the bytes written before the suspend

#### Scenario: Blocked cell across suspend
- **WHEN** a cell blocked in a system call is suspended and resumed
- **THEN** it waits in the same call again, and completes it once a descriptor is ready

#### Scenario: Parked record round trip
- **WHEN** socket records, a parked one among them, are encoded and decoded
- **THEN** the records are equal, and a truncated or out-of-range record is refused

### Requirement: Invalid socket records
A socket record whose fields are out of range SHALL make the snapshot
invalid.

#### Scenario: Out-of-range record
- **WHEN** a snapshot holds a socket record with a field out of range
- **THEN** decoding refuses it and no cell is created

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

### Requirement: Write-or-execute memory across snapshots
A resumed cell SHALL find its write-or-execute pages in the state they were
in, with their contents, and its write-or-execute ranges, whose untouched
pages SHALL still come zeroed on first touch, eagerly or lazily resumed.

#### Scenario: Code across a suspend
- **WHEN** a cell that wrote and ran code in write-or-execute memory is suspended, resumed, rewrites the code and runs it, and touches a new page of its range
- **THEN** the old code runs before the rewrite and the new one after, and the new page reads as zero

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

### Requirement: Snapshot checksum computed by the CPU
The kernel SHALL compute the CRC-32C of exported snapshots, and check it
when decoding, with the CPU's CRC instructions where the CPU has them
(AArch64 `CRC32C*` when `ID_AA64ISAR0_EL1` lists CRC32, x86_64 SSE4.2
`crc32` when `CPUID` lists SSE4.2), and with the portable table of
`rhizo-snapshot` otherwise. Both SHALL give the same checksum for any
bytes, so a file saved by one kernel loads in the other.

#### Scenario: Same checksum for any bytes
- **WHEN** the kernel's checksum and the portable table are computed over random bytes of every length from 0 to 63 and of about one and two pages, at every offset modulo 8
- **THEN** they are equal, and equal to the CRC-32C check value for `123456789`

#### Scenario: The function passed in is the one used
- **WHEN** a snapshot is exported with one checksum function and decoded with another that gives other values
- **THEN** decoding fails with a checksum error, and decoding with the first succeeds
