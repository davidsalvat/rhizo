## MODIFIED Requirements

### Requirement: libc startup and stdio calls
The kernel SHALL implement `mprotect` (writable and executable as the
requirement "Writable and executable memory" says), `prlimit64` and
`getrlimit`, `set_robust_list`, `fstat` and `newfstatat` of the standard
descriptors (character devices), `readlink` and `readlinkat` (no link
exists), `getuid`, `geteuid`, `getgid`, `getegid`, `getppid`,
`sigaltstack` and `madvise`, and SHALL decline `rseq` with `ENOSYS` without
reporting it. `futex` is specified by the `threads` capability.

#### Scenario: Relocation protection
- **WHEN** a static glibc program makes its relocated data read-only with `mprotect` at startup
- **THEN** the call succeeds and the program runs

#### Scenario: Writable and executable
- **WHEN** a cell asks `mprotect` for a writable and executable page it has
- **THEN** the call succeeds and the page becomes a write-or-execute page (spec `virtual-memory`), writing

## ADDED Requirements

### Requirement: Writable and executable memory
An anonymous private `mmap` with `PROT_WRITE|PROT_EXEC`, and `mprotect`
with both over reserved addresses, SHALL succeed without backing the
memory: the range SHALL be kept as a write-or-execute range, and each of
its pages SHALL get a zeroed 4 KiB write-or-execute page the first time
cell code or a system call on the cell's behalf touches it. `mprotect`
with both over mapped pages SHALL keep their frames and make them
write-or-execute pages.

#### Scenario: A JIT's code range
- **WHEN** a cell reserves 512 MiB with `PROT_NONE`, makes it writable and executable with `mprotect`, and writes and runs code in two of its pages
- **THEN** both calls succeed, the code runs, and the cell holds two pages of that range

#### Scenario: A system call fills untouched code memory
- **WHEN** a cell reads from a file into write-or-execute memory it never touched
- **THEN** the read succeeds and the pages hold the file's bytes

### Requirement: Write-or-execute ranges
`munmap`, `MAP_FIXED`, and `mprotect` with any protection other than
writable and executable SHALL first drop the part of the write-or-execute
ranges they cover. A cell SHALL have at most 64 ranges; a call that would
need more, a new range or a split, SHALL fail with `ENOMEM` and change
nothing.

#### Scenario: Unmapped part
- **WHEN** a cell unmaps part of a write-or-execute range it never touched, then touches it
- **THEN** it is killed with a fault report, and the rest of the range still comes zeroed on first touch

#### Scenario: Too many ranges
- **WHEN** a cell splits its write-or-execute memory into more than 64 separate ranges
- **THEN** the call that would make the 65th fails with `-ENOMEM` and the ranges are unchanged

### Requirement: Code that writes its own page
A write by an instruction to the write-or-execute page it runs from cannot
complete without the page being writable and executable at once: it SHALL
end the cell with a fault report.

#### Scenario: Self-modifying instruction
- **WHEN** code running from a write-or-execute page stores into that same page
- **THEN** the cell is killed with a fault report, and other cells run on
