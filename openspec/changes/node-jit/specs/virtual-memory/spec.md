## ADDED Requirements

### Requirement: Write-or-execute pages
A cell page asked writable and executable SHALL carry a software bit in its
page-table entry and SHALL be, at every moment, either writing (writable or
copy-on-write, not executable) or running (read-only, executable); no
page-table entry of a cell SHALL allow both writing and instruction
fetches.

#### Scenario: Never both
- **WHEN** any sequence of mapping, protecting, forking, cloning, writing and fetching is applied to write-or-execute pages, in either entry format
- **THEN** after every step no leaf entry allows both writing and execution

#### Scenario: The bit in both formats
- **WHEN** a write-or-execute leaf is encoded in the x86_64 or AArch64 format and decoded
- **THEN** the flags come back with the write-or-execute bit, which the hardware ignores

### Requirement: Changing between writing and running
An instruction fetch from a writing page SHALL make it running, and a
write to a running page SHALL make it writing. Each change SHALL flush the
page's old translation from every CPU that may hold it before the access
is retried, and the change to running SHALL make the page's contents
visible to instruction fetches on every CPU.

#### Scenario: Code written then run
- **WHEN** a cell writes an instruction sequence into a write-or-execute page and jumps to it
- **THEN** the sequence runs and returns, and the page's entry is read-only and executable

#### Scenario: Code rewritten
- **WHEN** the cell then writes a different sequence into the same page and jumps to it
- **THEN** the new sequence runs, never the old one

#### Scenario: Written by one thread, run by another
- **WHEN** one thread of a cell writes code into a page that another thread, on another CPU, runs
- **THEN** both make progress and the running thread runs only code completely written
