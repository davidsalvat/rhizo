# linux-abi Specification

## Purpose
Defines the Linux system-call interface cells use: which calls exist, how they
behave, and what happens with calls Rhizo does not implement.

## Requirements

### Requirement: Linux system-call convention
Cells SHALL invoke system calls with the Linux convention of their
architecture (`SVC #0` with the number in `x8` on AArch64, `SYSCALL` with the
number in `rax` on x86_64), and receive results or negated `errno` values as
Linux does.

#### Scenario: Same binary semantics as Linux
- **WHEN** a test program runs on Rhizo and on Linux on the same architecture
- **THEN** its output and exit status are the same

### Requirement: Supported calls
The kernel SHALL implement `write`, `writev`, `exit`, `exit_group`, `brk`,
`mmap` (anonymous private), `munmap`, `getpid`, `gettid`,
`set_tid_address`, `uname`, `clock_gettime`, `getrandom`, `rt_sigaction`,
`rt_sigprocmask`, `ioctl` and, on x86_64, `arch_prctl`, with Linux semantics
for the cases cells use in H2.

#### Scenario: Console output
- **WHEN** a cell writes to file descriptor 1 or 2
- **THEN** the bytes appear on the boot console and the call returns their count

#### Scenario: Bad pointer
- **WHEN** a cell passes a buffer that is not mapped in its address space
- **THEN** the call returns `-EFAULT` and the kernel keeps running

#### Scenario: Heap growth
- **WHEN** a cell grows its program break with `brk` and writes to the new memory
- **THEN** the write succeeds and the new memory reads as zero before it

### Requirement: Unsupported calls are visible
Any other system call SHALL return `-ENOSYS` and the kernel SHALL print the
call's name or number and the cell once, so missing calls are found quickly.
Numbers from 1023 up, past every Linux call, SHALL share one report, so a
cell cannot make the kernel print without end.

#### Scenario: Unknown call
- **WHEN** a cell invokes a system call Rhizo does not implement
- **THEN** it receives `-ENOSYS` and the console shows `unimplemented syscall` with the call

#### Scenario: Endless unknown numbers
- **WHEN** a cell invokes many different call numbers past every Linux one
- **THEN** each receives `-ENOSYS` and the console shows one report for all of them

### Requirement: Scheduling calls
The kernel SHALL implement `sched_yield`, `nanosleep` and `clock_nanosleep`
(relative sleeps on `CLOCK_MONOTONIC` and `CLOCK_REALTIME`, and absolute
`TIMER_ABSTIME` sleeps) with Linux semantics; a bad pointer returns `-EFAULT`
and an invalid time `-EINVAL`.

#### Scenario: Invalid sleep
- **WHEN** a cell calls `nanosleep` with nanoseconds outside 0–999,999,999
- **THEN** it receives `-EINVAL`

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

### Requirement: Opening and closing files
The kernel SHALL implement `openat` (and `open` on x86_64) with `O_CREAT`,
`O_EXCL`, `O_TRUNC`, `O_APPEND` and `O_DIRECTORY`, and `close`, with Linux
semantics and error numbers.

#### Scenario: Missing file
- **WHEN** a cell opens a path that does not exist without `O_CREAT`
- **THEN** it receives `-ENOENT`

#### Scenario: Exclusive create
- **WHEN** a cell opens an existing path with `O_CREAT | O_EXCL`
- **THEN** it receives `-EEXIST`

### Requirement: File input and output
The kernel SHALL implement `read`, `write` and `writev` on files, `pread64`,
`pwrite64`, `lseek`, `ftruncate`, `fsync`, `dup`, `dup2`, `dup3` and
`fcntl` (`F_DUPFD`, `F_DUPFD_CLOEXEC`, `F_GETFD`, `F_SETFD`, `F_GETFL`,
`F_SETFL`) with Linux semantics.

#### Scenario: Shared offset
- **WHEN** a cell duplicates a descriptor, reads through one and then through the other
- **THEN** the second read continues where the first stopped

### Requirement: Names and metadata
The kernel SHALL implement `fstat` and `newfstatat` on files and directories
(and `stat`, `lstat` on x86_64), `faccessat`, `getdents64`, `mkdirat` (and
`mkdir`), `unlinkat` (and `unlink`, `rmdir`), `renameat`, `renameat2`
without flags (and `rename`), `getcwd` and `chdir`.

#### Scenario: Removing a non-empty directory
- **WHEN** a cell removes a directory that still has entries
- **THEN** it receives `-ENOTEMPTY`

### Requirement: Select as a sleep
The kernel SHALL implement `select` (x86_64) and `pselect6` with no
descriptors as a sleep for their timeout, and SHALL return `ENOSYS` when
descriptors are given.

#### Scenario: Timer select
- **WHEN** a cell calls `select(0, NULL, NULL, NULL, &timeout)`
- **THEN** it sleeps for the timeout and receives 0

### Requirement: Reserved mappings
An anonymous private `mmap` with `PROT_NONE` SHALL reserve its addresses
without memory. `mprotect` with an accessible protection on reserved
addresses SHALL back them with zeroed pages that have that protection
(how glibc makes thread stacks with a guard page and per-thread malloc
arenas); the pages it does not cover stay inaccessible.

#### Scenario: Stack with a guard page
- **WHEN** a cell reserves 1 MiB with `PROT_NONE` and makes all but its lowest page readable and writable
- **THEN** the pages read as zero and keep what is written, and touching the lowest page kills the cell with a fault report
