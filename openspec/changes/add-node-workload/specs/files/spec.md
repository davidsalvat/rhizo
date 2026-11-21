# Spec Delta

## MODIFIED Requirements

### Requirement: Bounded file data
A cell SHALL hold at most 256 MiB of file data; a write past that bound
SHALL fail with `ENOSPC`, and a file SHALL NOT grow past 256 MiB (`EFBIG`).

#### Scenario: Out of file space
- **WHEN** a cell holding 256 MiB of file data writes a new block
- **THEN** the write fails with `ENOSPC` and the kernel keeps running

#### Scenario: A runtime larger than 64 MiB
- **WHEN** a cell is given a 121 MiB executable among its files
- **THEN** it holds the whole file and can start it

### Requirement: File data takes its own size in memory
A cell's file data SHALL take at most its size in kernel memory plus a small
bookkeeping overhead (under 1/16), so the 256 MiB bound on file data is also
a bound on the memory it uses.

#### Scenario: A 16 MiB file
- **WHEN** a cell writes a 16 MiB file and holds it
- **THEN** free memory drops by less than 17 MiB, cell memory included
