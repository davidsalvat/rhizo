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
- **WHEN** a valid snapshot with bytes changed or cut, or random bytes behind a valid header, are read and resumed
- **THEN** they are refused, or the resumed cell holds the properties above

## ADDED Requirements

### Requirement: Write-or-execute pages stay the cell's
Changing a write-or-execute page between writing and running SHALL change
only the cell's own entry: a page whose tables or frame another cell
shares SHALL be changed in tables of the cell's own, and written only in
a frame of its own.

#### Scenario: Code pages after a fork
- **WHEN** copies of a forked cell each rewrite and run the code in a write-or-execute page they share
- **THEN** each runs its own code, and the others' pages are unchanged

### Requirement: Cache maintenance a cell may do
On AArch64 a cell SHALL be able to read `CTR_EL0` and clean and invalidate
caches by address (`DC CVAU`, `IC IVAU`) on its own memory without a
system call, as on Linux, on every CPU it runs on.

#### Scenario: Code made visible by the cell
- **WHEN** a cell writes code and calls GCC's `__clear_cache` on it before running it
- **THEN** the cell keeps running and the code runs
