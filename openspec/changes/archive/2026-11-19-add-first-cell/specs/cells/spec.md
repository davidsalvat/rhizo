# Spec Delta

## Purpose

Defines what a cell is: an isolated address space running a static Linux
program in user mode, how it is created from an executable, how it ends, and
how it is kept from the kernel and from other cells.

## ADDED Requirements

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
