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

### Requirement: Inaccessible pages are freed
`mprotect` with `PROT_NONE` (and `MAP_FIXED` with `PROT_NONE`) SHALL make
pages inaccessible by freeing them: their contents are not kept, and in the
mapping area their addresses stay reserved.

#### Scenario: A gap between segments
- **WHEN** a cell makes a mapped page inaccessible with `mprotect` and later readable and writable again
- **THEN** the page holds no memory in between and reads as zero afterwards

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
`sysinfo`, `sched_getaffinity` (one CPU) and `fadvise64` (accepted,
nothing to do), as coreutils and CPython call them at start-up. `sysinfo`
SHALL describe the calling cell only: uptime, the cell's memory quota (at
most the memory cells share) as total memory, what the cell's own use
leaves of it as free memory, its threads as processes, no swap and no
load.

#### Scenario: Number of CPUs
- **WHEN** a cell asks `sched_getaffinity` for its CPUs with a buffer of 128 bytes
- **THEN** it receives 8 and a mask with CPU 0 only

#### Scenario: Memory of the cell
- **WHEN** a cell with a 64 MiB memory quota and three threads calls `sysinfo`
- **THEN** it reads 64 MiB of total memory, its quota less what it uses as free memory, and 3 processes

### Requirement: Descriptor ioctls
`ioctl` SHALL implement `FIONBIO` (set or clear a descriptor's
non-blocking mode, as `fcntl(F_SETFL)` with `O_NONBLOCK` does), `FIOCLEX`
and `FIONCLEX` (set or clear close-on-exec) on any descriptor, with `EBADF`
for a descriptor that is not open and `EFAULT` for an unreadable argument;
any other request SHALL return `ENOTTY`.

#### Scenario: Socket with a timeout
- **WHEN** a cell switches a descriptor to non-blocking mode with `FIONBIO`, as CPython does for a socket with a timeout
- **THEN** the call succeeds and `fcntl(F_GETFL)` reports `O_NONBLOCK`; switched back, it does not

#### Scenario: Close-on-exec
- **WHEN** a cell calls `FIOCLEX` then `FIONCLEX` on a descriptor
- **THEN** `fcntl(F_GETFD)` reports `FD_CLOEXEC` after the first and not after the second

#### Scenario: Other requests
- **WHEN** a cell asks for a terminal's attributes with `TCGETS`
- **THEN** the call returns `-ENOTTY`

### Requirement: Time of day and clock resolution
The kernel SHALL implement `gettimeofday` (the realtime clock in seconds
and microseconds, an empty time zone; either pointer may be null) and
`clock_getres` (one nanosecond for every clock `clock_gettime` reads,
`EINVAL` for another), as libuv calls them; a bad pointer SHALL return
`EFAULT`.

#### Scenario: Time of day
- **WHEN** a cell calls `gettimeofday` with a buffer for the time and one for the time zone
- **THEN** it receives 0, the realtime clock's seconds and microseconds, and a zeroed time zone

#### Scenario: Resolution of an unknown clock
- **WHEN** a cell calls `clock_getres` for a clock `clock_gettime` does not read
- **THEN** it receives `-EINVAL`

### Requirement: Capabilities and thread names
`capget` SHALL report that a cell holds no capabilities: versions 1 to 3 of
the header get empty sets, another version is answered with version 3 in
the header and `EINVAL`. `prctl` SHALL accept `PR_SET_NAME` (the name is
not kept) and `PR_GET_NAME` (an empty name), and return `EINVAL` for any
other option.

#### Scenario: Capabilities of a cell
- **WHEN** a cell calls `capget` with a version 3 header
- **THEN** it receives 0 and empty effective, permitted and inheritable sets

#### Scenario: A thread names itself
- **WHEN** a cell calls `prctl` with `PR_SET_NAME`
- **THEN** it receives 0

### Requirement: Scheduling policy of threads
`sched_getparam` and `sched_getscheduler` SHALL report the normal policy
and priority 0 for the caller (0) or one of its threads, `ESRCH` for any
other thread and `EINVAL` for a negative one.

#### Scenario: Policy of another thread
- **WHEN** a cell calls `sched_getscheduler` for a thread that is not one of its own
- **THEN** it receives `-ESRCH`

### Requirement: Declined calls
`io_uring_setup` SHALL return `-ENOSYS` without being reported as an
unsupported call, as `rseq` does: libraries that try it (libuv) fall back to
calls cells have.

#### Scenario: libuv probes io_uring
- **WHEN** a cell calls `io_uring_setup`
- **THEN** it receives `-ENOSYS` and the console shows nothing

### Requirement: Hardware capabilities in the auxiliary vector
On AArch64 the auxiliary vector of every cell SHALL carry `AT_HWCAP` with
`HWCAP_FP`, `HWCAP_ASIMD` and `HWCAP_CPUID`, and no capability a cell
cannot use. On x86_64 it SHALL stay without `AT_HWCAP`: programs read the
CPU with `CPUID`.

#### Scenario: A program checks its capabilities
- **WHEN** a static glibc program reads `getauxval(AT_HWCAP)` in a cell on AArch64
- **THEN** it finds FP, Advanced SIMD and `HWCAP_CPUID`

### Requirement: ID registers read at EL0
A read of an AArch64 ID register by cell code (`MRS` with op0 3, op1 0,
CRn 0, CRm 0 to 7) SHALL be answered by the kernel as Linux answers it:
`MIDR_EL1` and `REVIDR_EL1` as the CPU has them, `MPIDR_EL1` with only
bit 31 set, every other register as zero; the cell resumes after the
instruction.

#### Scenario: A program reads the ID registers
- **WHEN** a static glibc program reads `MIDR_EL1`, `MPIDR_EL1` and `ID_AA64ISAR0_EL1` in a cell on AArch64
- **THEN** it finds a non-zero `MIDR_EL1` and `MPIDR_EL1` equal to bit 31, prints what it prints on Linux and exits with 0

#### Scenario: OpenBLAS picks its kernels
- **WHEN** numpy loads OpenBLAS in a cell on AArch64, which reads `MIDR_EL1` when `HWCAP_CPUID` is set and otherwise opens files under `/proc` and `/sys`
- **THEN** it reads the register and computes, and does not fault

#### Scenario: Any other undefined instruction
- **WHEN** cell code runs an undefined instruction that is not such a read, or writes an ID register
- **THEN** the cell ends with a fault, as before

### Requirement: The monotonic clock follows real time
The clock `clock_gettime` reads SHALL advance at the rate of real time
within 0.05% on every VMM, with or without KVM: on x86_64, the TSC
frequency the kernel finds (from KVM's clock, CPUID, the PIT or the local
APIC timer) SHALL agree within 0.05% with 50 ms of the local APIC timer,
which counts at 1 GHz on KVM and QEMU.

#### Scenario: Under emulation
- **WHEN** the kernel runs on QEMU without KVM (`microvm` or `q35`) and has found the TSC frequency
- **THEN** 50 ms of the local APIC timer converts to 50 ms of the clock within 0.05%

#### Scenario: A long wait measured in a cell
- **WHEN** a cell times, with `CLOCK_MONOTONIC`, a model that answers 65 s after its request
- **THEN** it reads at least 65 s, not 0.3 to 1.5% less

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
