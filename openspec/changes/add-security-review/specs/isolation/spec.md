# Spec Delta

## ADDED Requirements

### Requirement: The kernel half is out of every cell's reach
A cell SHALL NOT read, write, map, unmap or change the permissions of
kernel memory, by its own instructions or through any system call. A cell's
addresses end at `USER_END` (`0x7fff_ffff_f000`, as Linux's
`TASK_SIZE_MAX`): every copy between a cell and the kernel SHALL refuse a
range that reaches past it, whatever the page tables say, and no mapping
call SHALL change an entry for an address past it.

#### Scenario: Kernel pointer passed to a call
- **WHEN** a cell passes a kernel, non-canonical or last-page address as the buffer, path or result of any call that takes one
- **THEN** the call returns `-EFAULT` and kernel memory is unchanged

#### Scenario: Kernel range passed to mprotect or munmap
- **WHEN** a cell asks `mprotect` for any permissions, or `munmap`, on a range in the kernel half
- **THEN** `mprotect` returns `-ENOMEM`, `munmap` returns `-EINVAL`, and no kernel page-table entry changes

#### Scenario: Writing kernel memory
- **WHEN** cell code writes to a kernel address
- **THEN** the cell is killed with a fault report naming the address and a write, and the kernel keeps running cells

### Requirement: Hostile calls get answers, not crashes
Whatever call number and arguments a cell passes, the kernel SHALL answer
with a value or a negated Linux `errno`, SHALL NOT panic, and SHALL leave
kernel memory unchanged; once the cell ends, every frame it held SHALL be
free again.

#### Scenario: Random calls
- **WHEN** a cell makes 20000 system calls with numbers and arguments from a fixed-seed generator, next to another cell
- **THEN** every result is a value or an errno, both cells finish, and running it again leaves memory as it was

### Requirement: State the kernel loads stays the cell's
The kernel SHALL refuse values that would fault in the kernel itself when
it enters the cell: a thread pointer past `USER_END` (`arch_prctl` answers
`-EPERM`), an executable segment past `USER_END`, and a snapshot whose
program counter (or, on x86_64, thread pointer) is past `USER_END` or whose
`MXCSR` has reserved bits set.

#### Scenario: Kernel thread pointer
- **WHEN** a cell on x86_64 calls `arch_prctl(ARCH_SET_FS)` with a kernel or non-canonical address
- **THEN** it receives `-EPERM` and its thread pointer is unchanged

#### Scenario: Tampered snapshot
- **WHEN** the bytes of a snapshot hold a kernel program counter
- **THEN** reading them fails with a format error and nothing is resumed

### Requirement: Bounded kernel work per call
The kernel runs a system call to its end without preemption, so no call
SHALL take time proportional to the size of an empty range: walks over a
range SHALL skip unmapped space one empty table entry at a time, and
`mprotect` SHALL stop at the first page it cannot bring in. `mmap` and
`brk` SHALL refuse memory that cannot be had before zeroing any of it.

#### Scenario: Call storm
- **WHEN** a cell calls `munmap` and `mprotect` over terabytes of unmapped space without pause, next to another cell
- **THEN** the other cell runs to its end

### Requirement: Bounded kernel memory per cell
The kernel memory a cell's actions take SHALL be bounded or counted like
its pages: at most 4096 files and directories per cell, each created only
when memory for it can be had (`-ENOSPC` otherwise); a `getdents64` answer
of at most 64 KiB; at most 1 MiB of captured output; and one console
report per unknown call number.

#### Scenario: Endless names
- **WHEN** cells make directories with the longest names until refused, while another cell takes all memory it can
- **THEN** each is refused with `-ENOSPC`, all end, the kernel keeps running, and every frame comes back

### Requirement: No data crosses cells through memory
Memory given to a cell (a fresh mapping, its heap, the private copy of a
forked page, a resumed page) SHALL hold nothing written by another cell,
and what an ended cell mapped SHALL be mapped for no other cell.

#### Scenario: Recycled frames
- **WHEN** a cell fills 16 MiB with a pattern and exits, then a new cell, a forked child or a resumed cell maps 16 MiB and grows its heap
- **THEN** every byte it reads there is zero

#### Scenario: Shared page made writable
- **WHEN** copies of a forked cell each make a read-only page they share writable with `mprotect` and write to it
- **THEN** each reads back its own write, and the others' pages are unchanged

#### Scenario: After the end
- **WHEN** a cell reads the address where an ended cell had a mapping
- **THEN** it is killed with a fault report
