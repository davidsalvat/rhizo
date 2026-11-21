# Spec Delta

## ADDED Requirements

### Requirement: Memory quota per cell
The host SHALL be able to set how much memory a cell's pages and page
tables may take. An allocation past it SHALL fail as when memory runs out
(`mmap` and `brk` answer `-ENOMEM`), and SHALL NOT take memory from other
cells or the kernel.

#### Scenario: Hog with a quota
- **WHEN** a cell with an 8 MiB memory quota takes memory until refused, next to a cell without quota that asks for 16 MiB
- **THEN** the first is refused with `-ENOMEM` having taken at most 8 MiB, the other gets all it asked for, and every frame comes back when both end

#### Scenario: Holes of a mapped file count
- **WHEN** a cell with an 8 MiB memory quota maps a one-byte file of its own 1 MiB at a time until refused, next to a cell without quota that asks for 16 MiB
- **THEN** the pages past the file's end count in its quota: it is refused with `-ENOMEM` having taken at most 8 MiB, and the other gets all it asked for

### Requirement: CPU time quota per cell
The host SHALL be able to set how much CPU time a cell may use over its
life, its system calls included. A cell past it SHALL be ended at the end
of the time slice that used it up, with a report in its output.

#### Scenario: Spinning past its quota
- **WHEN** a cell with a 30 ms CPU quota spins forever next to another cell
- **THEN** it ends with `cell N killed: CPU time quota used up` and the other cell runs to its end

### Requirement: Descriptor and socket quotas per cell
The host SHALL be able to set how many descriptors (at most 1024) and
sockets a cell may hold; without a quota from the host, a cell SHALL hold
at most 256 sockets. A new descriptor past the first SHALL fail with
`-EMFILE`, a new socket past the second with `-ENFILE`, and
`getrlimit`/`prlimit64` SHALL report the cell's quotas on descriptors,
memory and CPU time.

#### Scenario: A bounded share of sockets by default
- **WHEN** a cell for which the host set no quota, and its fork, make sockets until refused
- **THEN** each gets 256 sockets then `-ENFILE`, so no cell holds more than an eighth of the kernel's 4096 sockets with the server ends of its own connections

#### Scenario: Opening until refused
- **WHEN** a cell with quotas of 16 descriptors and 3 sockets makes sockets until refused, closes them, then opens files until refused
- **THEN** it gets 3 sockets then `-ENFILE`, 13 files then `-EMFILE`, and `RLIMIT_NOFILE` reads 16

### Requirement: Quotas are cell state
A cell's quotas SHALL travel with it: a fork and a copy of a template SHALL
have the cell's quotas and none of its usage, and a resumed snapshot SHALL
have the cell's quotas and the CPU time it had used.

#### Scenario: Copies keep the quotas
- **WHEN** a cell with descriptor and socket quotas is forked, frozen and copied, and suspended and resumed, and each copy opens until refused
- **THEN** every copy stops at the same quotas as the cell
