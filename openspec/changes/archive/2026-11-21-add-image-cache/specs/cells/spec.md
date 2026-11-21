# Spec Delta

## ADDED Requirements

### Requirement: Executables kept loaded
The kernel SHALL load the segments of an executable it spawns once and
keep that space, never run, for later spawns of the same executable (at
most four executables, the least recently spawned dropped first). A later
spawn SHALL start its cell on a copy of the kept space with tables of its
own, sharing its read-only pages and with a private copy of every writable
page; the cell SHALL be the one a load would make.

#### Scenario: Spawned again
- **WHEN** a cell of an executable writes its static data and ends, and two more cells of it are spawned
- **THEN** each of them finds the executable's values and sees only its own writes

#### Scenario: A cell from a kept executable
- **WHEN** a cell started from a kept executable is suspended, resumed and forked
- **THEN** the resumed cell and its children run to their end as cells of a loaded executable do

### Requirement: Kept executables give way to cells
The frames of kept executables SHALL belong to the kernel and SHALL be
taken like a cell's, never from the kernel's reserve. When memory for cells
runs short, the kernel SHALL drop every kept executable before it refuses
memory to any cell.

#### Scenario: Memory runs short
- **WHEN** executables are kept and every other frame cells may take is in use
- **THEN** a new cell still spawns and runs to its end, and once it has ended and the kept executables are dropped, free memory is what it was before

## MODIFIED Requirements

### Requirement: Cell exit
A cell SHALL end when it calls `exit` or `exit_group`, and its exit status
SHALL be reported to the kernel; its memory and page tables SHALL be returned
to the frame allocator.

#### Scenario: Exit status
- **WHEN** a cell of an executable already kept loaded calls `exit_group(3)`
- **THEN** the kernel reports exit status 3 and the free frame count returns to its value before the spawn

### Requirement: Memory pressure is the cell's error
When cells together exhaust memory, the kernel SHALL keep a reserve for
itself and SHALL report the shortage to the cell that asked (`ENOMEM`,
`ENOSPC`, or a fault on a page it cannot get) or to the host (`spawn`,
`fork`, `suspend` or eager `resume` fails and changes nothing). The kernel
SHALL NOT panic, and SHALL get every frame back when the cells end.

#### Scenario: Cells fill memory
- **WHEN** more cells than memory can hold each map and write 8 MiB, or write 16 MiB of files
- **THEN** some are refused, the others finish, the kernel keeps running, and once all have ended and the kept executables are dropped, free memory is what it was before
