# Spec Delta

## MODIFIED Requirements

### Requirement: Bounded file data
A cell SHALL hold at most 128 MiB of file data; a write past that bound
SHALL fail with `ENOSPC`, and a file SHALL NOT grow past 128 MiB (`EFBIG`).

#### Scenario: Out of file space
- **WHEN** a cell holding 128 MiB of file data writes a new block
- **THEN** the write fails with `ENOSPC` and the kernel keeps running

#### Scenario: An interpreter with its packages
- **WHEN** a cell is given the packages cell's files (CPython, its standard library, numpy and requests: about 65 MiB)
- **THEN** it holds them and has room left to write files of its own

### Requirement: File data takes its own size in memory
A cell's file data SHALL take at most its size in kernel memory plus a small
bookkeeping overhead (under 1/16), so the 128 MiB bound on file data is also
a bound on the memory it uses.

#### Scenario: A 16 MiB file
- **WHEN** a cell writes a 16 MiB file and holds it
- **THEN** free memory drops by less than 17 MiB, cell memory included
