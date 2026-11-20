# Spec Delta

## ADDED Requirements

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
