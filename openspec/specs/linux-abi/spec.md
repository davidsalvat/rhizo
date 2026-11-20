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
descriptors as a sleep for their timeout, and with descriptors as waits on
them (requirement "Waiting on descriptors").

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

### Requirement: Pipes and event counters
The kernel SHALL implement `pipe2` (and `pipe` on x86_64) and `eventfd2`
(and `eventfd` on x86_64) with Linux semantics: a pipe holds up to 64 KiB,
reading an empty pipe whose write ends are all closed returns 0, writing to
a pipe whose read ends are all closed fails with `EPIPE`; an event counter
adds on `write` and returns and clears its value on `read` (one at a time
with `EFD_SEMAPHORE`).

#### Scenario: Pipe round trip
- **WHEN** a cell writes bytes into a pipe and reads its other end
- **THEN** it reads the same bytes in order

#### Scenario: End of a pipe
- **WHEN** a cell closes the write end of an empty pipe and reads the read end
- **THEN** the read returns 0

### Requirement: Non-blocking descriptors
`O_NONBLOCK` SHALL be settable with `fcntl(F_SETFL)` and with the
`*_NONBLOCK` flags of `pipe2`, `eventfd2`, `socket`, `socketpair` and
`accept4`, and SHALL make a call that would wait fail with `EAGAIN`
(`EINPROGRESS` for `connect`) instead.

#### Scenario: Empty non-blocking pipe
- **WHEN** a cell reads an empty pipe whose read end is non-blocking
- **THEN** it receives `-EAGAIN` at once

### Requirement: Waiting on descriptors
The kernel SHALL implement `poll` (x86_64), `ppoll`, `select`, `pselect6`,
`epoll_create1` (and `epoll_create` on x86_64), `epoll_ctl`, `epoll_wait`
(x86_64) and `epoll_pwait` with Linux semantics for the events `POLLIN`,
`POLLOUT`, `POLLERR`, `POLLHUP`, `POLLRDHUP` and `POLLNVAL` (and their
`EPOLL*` equivalents), level-triggered, with `EPOLLONESHOT`; a call with
nothing ready waits until a descriptor is ready or its timeout ends.

#### Scenario: Timeout with nothing ready
- **WHEN** a cell polls an empty pipe's read end with a 10 ms timeout
- **THEN** the call returns 0 after at least 10 ms

#### Scenario: Ready descriptor
- **WHEN** a cell waits with `epoll_pwait` on a socket that another cell then writes to
- **THEN** the call returns that socket with `EPOLLIN` and the data given at `epoll_ctl`

#### Scenario: Regular file in epoll
- **WHEN** a cell adds a regular file to an `epoll` instance
- **THEN** it receives `-EPERM`

### Requirement: Socket calls
The kernel SHALL implement `socket`, `socketpair`, `bind`, `listen`,
`accept` (x86_64), `accept4`, `connect`, `sendto`, `recvfrom`, `sendmsg`,
`recvmsg`, `shutdown`, `getsockname`, `getpeername`, `setsockopt` and
`getsockopt`, and `read`, `write`, `readv` and `writev` on sockets, for
`AF_INET` (`SOCK_STREAM`, `SOCK_DGRAM`) and `AF_UNIX` (`SOCK_STREAM`), with
Linux error numbers; other families fail with `EAFNOSUPPORT` and other
types with `ESOCKTNOSUPPORT`.

#### Scenario: Same behavior as Linux
- **WHEN** a C program doing a TCP echo over 127.0.0.1 with `poll` and `epoll`, a pipe, an event counter, UDP datagrams and a Unix socket runs as a cell and on Linux
- **THEN** both print the same output and exit with the same status

#### Scenario: No listener
- **WHEN** a cell connects to a loopback port nobody listens on
- **THEN** it receives `-ECONNREFUSED`

### Requirement: Socket options
`setsockopt` SHALL accept `SO_REUSEADDR`, `SO_REUSEPORT`, `SO_KEEPALIVE`,
`SO_RCVBUF`, `SO_SNDBUF`, `SO_RCVTIMEO`, `SO_SNDTIMEO`, `SO_LINGER`,
`SO_BROADCAST`, `TCP_NODELAY`, `TCP_KEEPIDLE`, `TCP_KEEPINTVL` and
`TCP_KEEPCNT`; `getsockopt` SHALL report them and `SO_ERROR`, `SO_TYPE`,
`SO_DOMAIN` and `SO_ACCEPTCONN`. `SO_RCVTIMEO` and `SO_SNDTIMEO` SHALL
bound a blocking wait, which then fails with `EAGAIN`.

#### Scenario: Receive timeout
- **WHEN** a cell sets a 10 ms `SO_RCVTIMEO` and receives on a socket with no data
- **THEN** the call fails with `-EAGAIN` after at least 10 ms

### Requirement: Calls CPython uses
The kernel SHALL implement the calls a static CPython makes at start-up
and while answering a request that earlier milestones did not need, with
Linux semantics: `time` on x86_64 (seconds since the epoch, also stored
through a non-null pointer, `-EFAULT` when that pointer is not writable).

#### Scenario: Seconds since the epoch
- **WHEN** an x86_64 cell calls `time` with a pointer to writable memory
- **THEN** it receives the real-time clock's whole seconds, and the same value is stored at the pointer

### Requirement: A stack that grows
A cell's stack SHALL grow on demand, as a Linux process's does: the first
touch of an unmapped page within 8 MiB below the top of the stack (the
`RLIMIT_STACK` that `getrlimit` reports), by the cell or by a system call
writing to its memory, SHALL map a zeroed page there, counted against the
cell's memory like any other. Anonymous mappings SHALL NOT be placed in
that range or in the guard page below it.

#### Scenario: Deep recursion
- **WHEN** a static glibc program recurses through 4 MiB of stack frames and then passes an untouched 256 KiB stack buffer to `getcwd`
- **THEN** it prints what it prints on Linux, exits with 0, and every frame comes back when it ends

#### Scenario: Beyond the limit
- **WHEN** a cell touches an unmapped address below the 8 MiB stack limit
- **THEN** the cell is killed with a fault report, as before
