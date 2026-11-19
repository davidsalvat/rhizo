# files Specification

## Purpose
Defines the file system each cell has: in memory, private to the cell,
carried by its snapshots and shared copy-on-write with its forks.

## Requirements

### Requirement: Files in a cell
Each cell SHALL have its own tree of directories and regular files, created,
read, written, listed, renamed and removed through Linux system calls with
Linux semantics.

#### Scenario: Same behavior as Linux
- **WHEN** a C program that creates, writes, reads, lists, renames and removes files runs as a cell and in an empty directory on Linux
- **THEN** both print the same output and exit with the same status

#### Scenario: Unlinked while open
- **WHEN** a cell unlinks a file it has open and then reads it through the descriptor
- **THEN** it reads the file's data, and the data is freed when the descriptor is closed

### Requirement: Files are private
A cell SHALL NOT see or change another cell's files.

#### Scenario: Two cells, same path
- **WHEN** two cells write different contents to the same path
- **THEN** each reads its own contents

### Requirement: Bounded file data
A cell SHALL hold at most 64 MiB of file data; a write past that bound
SHALL fail with `ENOSPC`, and a file SHALL NOT grow past 64 MiB (`EFBIG`).

#### Scenario: Out of file space
- **WHEN** a cell holding 64 MiB of file data writes a new block
- **THEN** the write fails with `ENOSPC` and the kernel keeps running

### Requirement: Initial files
The kernel SHALL let the host give a cell files when it spawns it.

#### Scenario: Input file
- **WHEN** a cell is spawned with a file and reads it
- **THEN** it reads the given contents

### Requirement: File data takes its own size in memory
A cell's file data SHALL take at most its size in kernel memory plus a small
bookkeeping overhead (under 1/16), so the 64 MiB bound on file data is also
a bound on the memory it uses.

#### Scenario: A 16 MiB file
- **WHEN** a cell writes a 16 MiB file and holds it
- **THEN** free memory drops by less than 17 MiB, cell memory included
