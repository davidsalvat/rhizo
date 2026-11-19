# Spec Delta

## ADDED Requirements

### Requirement: libc startup and stdio calls
The kernel SHALL implement `mprotect` (W^X kept), `prlimit64` and
`getrlimit`, `set_robust_list`, `fstat` and `newfstatat` of the standard
descriptors (character devices), `readlink` and `readlinkat` (no link
exists), `getuid`, `geteuid`, `getgid`, `getegid`, `getppid`,
`sigaltstack`, `madvise` and a single-threaded `futex`, and SHALL decline
`rseq` with `ENOSYS` without reporting it.

#### Scenario: Relocation protection
- **WHEN** a static glibc program makes its relocated data read-only with `mprotect` at startup
- **THEN** the call succeeds and the program runs

#### Scenario: Writable and executable
- **WHEN** a cell asks `mprotect` for a writable and executable page
- **THEN** it receives `-EACCES`
