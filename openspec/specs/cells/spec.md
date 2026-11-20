# cells Specification

## Purpose
Defines what a cell is: an isolated address space running a static Linux
program in user mode, how it is created from an executable, how it ends, and
how it is kept from the kernel and from other cells.

## Requirements

### Requirement: Spawn a cell from a static executable
The kernel SHALL create a cell from a static 64-bit ELF executable for its
architecture: map each loadable segment with its permissions, zero its
`.bss`, build a stack with `argv`, `envp` and the auxiliary vector, and start
the program at its entry point in user mode.

#### Scenario: Hello
- **WHEN** the kernel spawns the `hello` test program
- **THEN** the console shows the program's output and the cell exits with status 0

#### Scenario: Rejected executables
- **WHEN** the image is not an ELF, is for another architecture, is dynamically linked, or has a segment that is both writable and executable
- **THEN** spawning fails with an error naming the reason and no cell is created

### Requirement: Cell isolation
Cell code SHALL run without privileges and SHALL NOT be able to read, write or
execute kernel memory or any memory not mapped into its own address space.

#### Scenario: Reading kernel memory
- **WHEN** a cell reads a kernel address
- **THEN** the cell is killed with a fault report and the kernel continues

### Requirement: Cell exit
A cell SHALL end when it calls `exit` or `exit_group`, and its exit status
SHALL be reported to the kernel; its memory and page tables SHALL be returned
to the frame allocator.

#### Scenario: Exit status
- **WHEN** a cell calls `exit_group(3)`
- **THEN** the kernel reports exit status 3 and the free frame count returns to its value before the spawn

### Requirement: Cell state is plain data
Everything needed to resume a cell (registers, address-space root, program
break, mapping cursor) SHALL be held in plain data with no pointers into
kernel memory, so it can be copied and serialized.

#### Scenario: Copying registers
- **WHEN** a cell's register state is copied bit for bit
- **THEN** the copy describes the same execution point

### Requirement: Large pages for large anonymous mappings
Anonymous mappings of at least 2 MiB SHALL be placed at 2 MiB-aligned
addresses and mapped with 2 MiB pages.

#### Scenario: 4 MiB mapping
- **WHEN** a cell maps 4 MiB of anonymous memory
- **THEN** the region is backed by two 2 MiB pages

### Requirement: Isolation between cells
Each cell SHALL have its own address space: the same virtual address in two
cells refers to different memory, and a cell SHALL NOT be able to read or
write memory mapped only in another cell. A fault in one cell SHALL NOT
affect any other cell.

#### Scenario: Same address, different memory
- **WHEN** two cells write different values at the same virtual address, yield to each other, and read it back
- **THEN** each reads its own value

#### Scenario: Another cell's mapping
- **WHEN** a cell touches an address that only another cell has mapped
- **THEN** it is killed with a fault report and the other cell is unaffected

### Requirement: Tagged address spaces
Switching between cells SHALL NOT flush translations of other cells when the
CPU supports address-space identifiers; a cell's translations SHALL be
removed when it ends, before its identifier is reused.

#### Scenario: Reused identifier
- **WHEN** more cells are created and ended than there are identifiers in use at once
- **THEN** every cell still sees only its own memory

### Requirement: Suspended cells use no memory but their snapshot
A suspended cell SHALL hold no frames, page tables or address-space tag; its
whole state SHALL be its snapshot.

#### Scenario: Many suspended cells
- **WHEN** 32 cells are suspended
- **THEN** no cell is live (no page table, frame or tag is held) and each snapshot can be resumed and run to completion

### Requirement: Floating-point state is per cell
Each cell SHALL have its own floating-point and SIMD registers, kept across
preemption, system calls, suspend/resume and fork, and never visible to
another cell.

#### Scenario: Two FP cells
- **WHEN** two cells hold different values in FP/SIMD registers across yields
- **THEN** each finds its own values

#### Scenario: FP across a snapshot
- **WHEN** a cell computing with FP/SIMD registers is suspended and resumed repeatedly
- **THEN** its result is the same as without suspension

### Requirement: Memory pressure is the cell's error
When cells together exhaust memory, the kernel SHALL keep a reserve for
itself and SHALL report the shortage to the cell that asked (`ENOMEM`,
`ENOSPC`, or a fault on a page it cannot get) or to the host (`spawn`,
`fork`, `suspend` or eager `resume` fails and changes nothing). The kernel
SHALL NOT panic, and SHALL get every frame back when the cells end.

#### Scenario: Cells fill memory
- **WHEN** more cells than memory can hold each map and write 8 MiB, or write 16 MiB of files
- **THEN** some are refused, the others finish, the kernel keeps running, and once all have ended free memory is what it was before

### Requirement: Thousands of idle cells
The kernel SHALL hold at least 1000 live idle cells forked from one
template at once, each costing a few pages beyond what it shares.

#### Scenario: 1000 idle cells
- **WHEN** a template of an idle cell is forked 1000 times and every child runs into its sleep
- **THEN** all 1000 are live, each costs less than 64 KiB of memory, and killing them returns all of it
