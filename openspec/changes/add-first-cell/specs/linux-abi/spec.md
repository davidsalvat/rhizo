# Spec Delta

## Purpose

Defines the Linux system-call interface cells use: which calls exist, how they
behave, and what happens with calls Rhizo does not implement.

## ADDED Requirements

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

#### Scenario: Unknown call
- **WHEN** a cell invokes a system call Rhizo does not implement
- **THEN** it receives `-ENOSYS` and the console shows `unimplemented syscall` with the call
