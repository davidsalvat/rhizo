# Spec Delta

## MODIFIED Requirements

### Requirement: libc startup and stdio calls
The kernel SHALL implement `mprotect` (W^X kept), `prlimit64` and
`getrlimit`, `set_robust_list`, `fstat` and `newfstatat` of the standard
descriptors (character devices), `readlink` and `readlinkat` (no link
exists), `getuid`, `geteuid`, `getgid`, `getegid`, `getppid`,
`sigaltstack` and `madvise`, and SHALL decline `rseq` with `ENOSYS` without
reporting it. `futex` is specified by the `threads` capability.

#### Scenario: Relocation protection
- **WHEN** a static glibc program makes its relocated data read-only with `mprotect` at startup
- **THEN** the call succeeds and the program runs

#### Scenario: Writable and executable
- **WHEN** a cell asks `mprotect` for a writable and executable page
- **THEN** it receives `-EACCES`

## ADDED Requirements

### Requirement: Reserved mappings
An anonymous private `mmap` with `PROT_NONE` SHALL reserve its addresses
without memory. `mprotect` with an accessible protection on reserved
addresses SHALL back them with zeroed pages that have that protection
(how glibc makes thread stacks with a guard page and per-thread malloc
arenas); the pages it does not cover stay inaccessible.

#### Scenario: Stack with a guard page
- **WHEN** a cell reserves 1 MiB with `PROT_NONE` and makes all but its lowest page readable and writable
- **THEN** the pages read as zero and keep what is written, and touching the lowest page kills the cell with a fault report
