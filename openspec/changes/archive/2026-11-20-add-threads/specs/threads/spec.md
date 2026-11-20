# Spec Delta

## ADDED Requirements

### Requirement: Threads of a cell
A cell SHALL run one or more threads that share its address space,
address-space tag, files, program break and mappings. Each thread SHALL have
its own registers, FP/SIMD state, stack and thread pointer (`TPIDR_EL0` on
AArch64, the `FS` base on x86_64).

#### Scenario: Shared memory
- **WHEN** a thread created with `clone` writes a value and another thread of the cell reads it
- **THEN** the reader sees the value

#### Scenario: Own thread pointer
- **WHEN** two threads are created with different `CLONE_SETTLS` values and run interleaved
- **THEN** each reads back its own thread pointer

### Requirement: Creating threads
The kernel SHALL implement `clone` and `clone3` for threads (`CLONE_VM`,
`CLONE_SIGHAND` and `CLONE_THREAD`). The new thread SHALL continue after
the call with the caller's registers and FP/SIMD state, `0` as result, the
given stack pointer (`clone3`: stack plus stack size) and, with
`CLONE_SETTLS`, the given thread pointer. The caller SHALL receive the new
thread's identifier.

#### Scenario: Identifier written for both
- **WHEN** a thread is created with `CLONE_PARENT_SETTID`, `CLONE_CHILD_SETTID` and `CLONE_CHILD_CLEARTID`
- **THEN** its identifier is written as a 32-bit value at both addresses, and its clear-tid address is the child address

#### Scenario: pthread_create
- **WHEN** a static glibc program calls `pthread_create`
- **THEN** the new thread runs its start routine with its own stack and thread pointer, and `pthread_create` returns 0

### Requirement: Refused clones
A `clone` or `clone3` that would create a process (without `CLONE_VM`,
`CLONE_SIGHAND` or `CLONE_THREAD`) SHALL return `-ENOSYS`. Flags other than
those above and `CLONE_FS`, `CLONE_FILES`, `CLONE_SYSVSEM` and
`CLONE_DETACHED`, a `clone3` size below 64 bytes or nonzero `clone3` fields
that are not supported SHALL return `-EINVAL`; an unmapped `clone3`
argument block `-EFAULT`. When the kernel cannot hold one more thread, the
call SHALL return `-EAGAIN`.

#### Scenario: Process clone
- **WHEN** a cell calls `clone` without `CLONE_VM`
- **THEN** it receives `-ENOSYS` and no thread is created

### Requirement: Thread pointers stay in the lower half
A `clone` or `clone3` with `CLONE_SETTLS` and a thread pointer outside the
lower half SHALL return `-EPERM`: it would fault in the kernel when loaded
into the x86_64 FS base.

#### Scenario: Kernel thread pointer
- **WHEN** a cell calls `clone` with `CLONE_SETTLS` and a thread pointer at or above `0x0000_8000_0000_0000`
- **THEN** it receives `-EPERM` and no thread is created

### Requirement: Thread identifiers
`gettid` SHALL return the calling thread's identifier. The first thread's
identifier SHALL be the cell's identifier at spawn; each new thread SHALL
get an identifier no other thread of the cell has had. A thread SHALL keep
its identifier across suspend, resume and fork. `getpid` SHALL return the
cell's identifier for every thread. `set_tid_address` SHALL set the calling
thread's clear-tid address and return its identifier.

#### Scenario: Distinct identifiers
- **WHEN** three threads of a cell call `gettid`
- **THEN** they receive three different positive values, and the first thread's equals `getpid`

### Requirement: Thread exit
`exit` SHALL end only the calling thread. If the thread has a clear-tid
address, the kernel SHALL write a 32-bit zero there and wake one futex
waiter on it (as `pthread_join` expects). When the last thread exits, the
cell SHALL end with that thread's status. `exit_group` SHALL end the cell
and all its threads.

#### Scenario: Join
- **WHEN** a thread waits with `FUTEX_WAIT` on the clear-tid address of another thread that then exits
- **THEN** the waiter wakes and reads 0 at that address

#### Scenario: Last thread
- **WHEN** every thread but the first has exited and the first calls `exit(4)`
- **THEN** the cell ends with status 4

#### Scenario: Exit group
- **WHEN** one thread calls `exit_group(2)` while others are blocked
- **THEN** the cell ends with status 2 and all its memory is returned

### Requirement: Futex wait and wake
The kernel SHALL implement `futex` with `FUTEX_WAIT`, `FUTEX_WAKE`,
`FUTEX_WAIT_BITSET` and `FUTEX_WAKE_BITSET`, with or without
`FUTEX_PRIVATE_FLAG`. A wait SHALL block the calling thread when the 32-bit
word still holds the expected value (else return `-EAGAIN`) until a wake on
the same address whose bitset intersects the waiter's (the wait returns 0)
or until its timeout (the wait returns `-ETIMEDOUT`). A wake SHALL wake up
to the given number of waiters and return how many it woke.

#### Scenario: Wait and wake
- **WHEN** one thread waits on a word and another changes the word and wakes one waiter on it
- **THEN** the wake returns 1 and the wait returns 0

#### Scenario: Value already changed
- **WHEN** a thread waits on a word that no longer holds the expected value
- **THEN** it receives `-EAGAIN` at once

#### Scenario: Bitsets
- **WHEN** a waiter with bitset 1 and a waiter with bitset 2 wait on one word and `FUTEX_WAKE_BITSET` with bitset 2 wakes all
- **THEN** only the second waiter wakes and the wake returns 1

### Requirement: Futex timeouts and errors
A `FUTEX_WAIT` timeout SHALL be relative on the monotonic clock; a
`FUTEX_WAIT_BITSET` timeout SHALL be absolute, on the monotonic clock or,
with `FUTEX_CLOCK_REALTIME`, the real-time clock. A misaligned address, an
empty bitset or an invalid timeout SHALL return `-EINVAL`, an unmapped word
`-EFAULT`; other operations SHALL return `-ENOSYS`.

#### Scenario: Timeout
- **WHEN** a thread waits 20 ms on a word nobody wakes
- **THEN** it receives `-ETIMEDOUT` after at least 20 ms

#### Scenario: Misaligned word
- **WHEN** a thread waits on an address that is not a multiple of 4
- **THEN** it receives `-EINVAL`
