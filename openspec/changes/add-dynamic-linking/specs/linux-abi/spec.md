# Spec Delta

## ADDED Requirements

### Requirement: File mappings
`mmap` with `MAP_PRIVATE` of a regular file open for reading SHALL map the
file's pages from a page-aligned offset: they read as the file, the bytes
past its end as zero, and a write (where `PROT_WRITE` allows it) SHALL
change only the cell's private copy, never the file or another mapping.
`munmap` and `mprotect` SHALL work on file mappings as on anonymous ones.

#### Scenario: A library mapped by the dynamic loader
- **WHEN** a cell maps a file read-only, maps its middle page writable over it with `MAP_FIXED`, writes that page and maps the file again
- **THEN** every mapping reads the file's bytes, only the written page holds the write, and the file is unchanged

### Requirement: Refused file mappings
A file mapping SHALL fail with `EACCES` for `PROT_WRITE` with `PROT_EXEC`
(W^X) or a descriptor open only for writing, `EINVAL` for an offset not
page aligned, `EBADF` for a descriptor that is not open, and `ENODEV` for
one that is not a regular file and for `MAP_SHARED`.

#### Scenario: Shared file mapping
- **WHEN** a cell maps a file with `MAP_SHARED`
- **THEN** it receives `-ENODEV`

### Requirement: Fixed mappings
`mmap` with `MAP_FIXED` SHALL map at the page-aligned address given, in
place of whatever was mapped there (anonymous memory, a file, or nothing);
an address that is not page aligned or a range past the cell's addresses
SHALL fail with `EINVAL`, and the first page with `EPERM`.

#### Scenario: Anonymous memory over a file
- **WHEN** a cell maps a file and then anonymous memory over its last page with `MAP_FIXED`
- **THEN** the call returns the address given and that page reads as zero

### Requirement: Start-up calls of distribution programs
The kernel SHALL implement `statx` (the fields `newfstatat` gives),
`sysinfo` (uptime and the memory cells share; no swap, one process),
`sched_getaffinity` (one CPU) and `fadvise64` (accepted, nothing to do),
as coreutils and CPython call them at start-up.

#### Scenario: Number of CPUs
- **WHEN** a cell asks `sched_getaffinity` for its CPUs with a buffer of 128 bytes
- **THEN** it receives 8 and a mask with CPU 0 only

## MODIFIED Requirements

### Requirement: Supported calls
The kernel SHALL implement `write`, `writev`, `exit`, `exit_group`, `brk`,
`mmap` (anonymous private, and private file mappings), `munmap`, `getpid`,
`gettid`, `set_tid_address`, `uname`, `clock_gettime`, `getrandom`,
`rt_sigaction`, `rt_sigprocmask`, `ioctl` and, on x86_64, `arch_prctl`, with
Linux semantics for the cases cells use in H2.

#### Scenario: Console output
- **WHEN** a cell writes to file descriptor 1 or 2
- **THEN** the bytes appear on the boot console and the call returns their count

#### Scenario: Bad pointer
- **WHEN** a cell passes a buffer that is not mapped in its address space
- **THEN** the call returns `-EFAULT` and the kernel keeps running

#### Scenario: Heap growth
- **WHEN** a cell grows its program break with `brk` and writes to the new memory
- **THEN** the write succeeds and the new memory reads as zero before it

### Requirement: Reserved mappings
An anonymous private `mmap` with `PROT_NONE` SHALL reserve its addresses
without memory. `mprotect` with an accessible protection on reserved
addresses SHALL back them with zeroed pages that have that protection
(how glibc makes thread stacks with a guard page and per-thread malloc
arenas); the pages it does not cover stay inaccessible. `mprotect` with
`PROT_NONE` (and `MAP_FIXED` with `PROT_NONE`) SHALL make pages
inaccessible by freeing them: their contents are not kept, and in the
mapping area their addresses stay reserved.

#### Scenario: Stack with a guard page
- **WHEN** a cell reserves 1 MiB with `PROT_NONE` and makes all but its lowest page readable and writable
- **THEN** the pages read as zero and keep what is written, and touching the lowest page kills the cell with a fault report

#### Scenario: A gap between segments
- **WHEN** a cell makes a mapped page inaccessible with `mprotect` and later readable and writable again
- **THEN** the page holds no memory in between and reads as zero afterwards
