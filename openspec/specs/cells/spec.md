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
- **WHEN** a cell of an executable already kept loaded calls `exit_group(3)`
- **THEN** the kernel reports exit status 3 and the free frame count returns to its value before the spawn

### Requirement: Cell state is plain data
Everything needed to resume a cell (each thread's registers, identifier,
clear-tid address and what it waits for; the address-space root, program
break and mapping cursor) SHALL be held in plain data with no pointers into
kernel memory, so it can be copied and serialized.

#### Scenario: Copying registers
- **WHEN** a cell's register state is copied bit for bit
- **THEN** the copy describes the same execution point

#### Scenario: Copying threads
- **WHEN** the thread table of a cell with blocked and runnable threads is copied
- **THEN** the copy has the same threads, in the same order, waiting for the same things

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
Each thread of each cell SHALL have its own floating-point and SIMD
registers, kept across preemption, system calls, thread switches,
suspend/resume and fork, and never visible to another thread or cell.

#### Scenario: Two FP cells
- **WHEN** two cells hold different values in FP/SIMD registers across yields
- **THEN** each finds its own values

#### Scenario: FP across a snapshot
- **WHEN** a cell computing with FP/SIMD registers is suspended and resumed repeatedly
- **THEN** its result is the same as without suspension

#### Scenario: Two FP threads
- **WHEN** two threads of one cell hold different values in FP/SIMD registers while they take turns
- **THEN** each finds its own values

### Requirement: Memory pressure is the cell's error
When cells together exhaust memory, the kernel SHALL keep a reserve for
itself and SHALL report the shortage to the cell that asked (`ENOMEM`,
`ENOSPC`, or a fault on a page it cannot get) or to the host (`spawn`,
`fork`, `suspend` or eager `resume` fails and changes nothing). The kernel
SHALL NOT panic, and SHALL get every frame back when the cells end.

#### Scenario: Cells fill memory
- **WHEN** more cells than memory can hold each map and write 8 MiB, or write 16 MiB of files
- **THEN** some are refused, the others finish, the kernel keeps running, and once all have ended and the kept executables are dropped, free memory is what it was before

### Requirement: Thousands of idle cells
The kernel SHALL hold at least 1000 live idle cells forked from one
template at once, each costing a few pages beyond what it shares.

#### Scenario: 1000 idle cells
- **WHEN** a template of an idle cell is forked 1000 times and every child runs into its sleep
- **THEN** all 1000 are live, each costs less than 64 KiB of memory, and killing them returns all of it

### Requirement: Executables kept loaded
The kernel SHALL load the segments of an executable it spawns once and
keep that space, never run, for later spawns of the same executable (at
most four executables, the least recently spawned dropped first). A later
spawn SHALL start its cell on a copy of the kept space with tables of its
own, sharing its read-only pages and with a private copy of every writable
page; the cell SHALL be the one a load would make.

#### Scenario: Spawned again
- **WHEN** a cell of an executable writes its static data and ends, and two more cells of it are spawned
- **THEN** each of them finds the executable's values and sees only its own writes

#### Scenario: A cell from a kept executable
- **WHEN** a cell started from a kept executable is suspended, resumed and forked
- **THEN** the resumed cell and its children run to their end as cells of a loaded executable do

### Requirement: Kept executables give way to cells
The frames of kept executables SHALL belong to the kernel and SHALL be
taken like a cell's, never from the kernel's reserve. When memory for cells
runs short, the kernel SHALL drop every kept executable before it refuses
memory to any cell.

#### Scenario: Memory runs short
- **WHEN** executables are kept and every other frame cells may take is in use
- **THEN** a new cell still spawns and runs to its end, and once it has ended and the kept executables are dropped, free memory is what it was before

### Requirement: The host wakes a sleeping cell
The host SHALL be able to end the sleeps of a live cell's threads and time
out their timed waits, as an event from outside the cell would; the cell
is then runnable at once. Waking a cell that does not exist SHALL report
that it does not.

#### Scenario: Woken worker
- **WHEN** the host wakes a `worker` sleeping for an hour after `ready`
- **THEN** it runs its one second of operations and exits with 0

#### Scenario: No such cell
- **WHEN** the host wakes a cell identifier that is not live
- **THEN** the call reports that no cell was woken

### Requirement: Executables from the cell's files
The kernel SHALL create a cell from a 64-bit ELF executable for its
architecture held in the cell's own files, given its path, `argv` and
`envp`: static or position independent (loaded at a fixed base),
statically or dynamically linked.

#### Scenario: Dynamic hello
- **WHEN** the host starts `/usr/bin/hello`, a C program linked dynamically with glibc, from files holding it, glibc's `ld.so` and `libc.so.6`
- **THEN** the cell prints what the same program prints on Linux, including that its auxiliary vector named an interpreter, and exits with 0

#### Scenario: No such executable
- **WHEN** the path names nothing, a directory, or a file that is not an ELF executable for the architecture
- **THEN** starting fails with an error naming the reason and no cell is created

### Requirement: Interpreters
For a dynamically linked executable the kernel SHALL also map the
interpreter its `PT_INTERP` names, from the cell's files, at the first
mapping address, and SHALL start the program at the interpreter's entry
with `AT_BASE` in the auxiliary vector next to the executable's
`AT_PHDR`, `AT_PHENT`, `AT_PHNUM` and `AT_ENTRY`.

#### Scenario: Started by its interpreter
- **WHEN** a position-independent executable naming an interpreter is started
- **THEN** its first instruction is the interpreter's entry, `AT_BASE` is where the interpreter was mapped and `AT_ENTRY` the executable's entry where it was mapped

#### Scenario: Missing interpreter
- **WHEN** the executable names an interpreter the cell's files do not hold
- **THEN** starting fails with an interpreter error, no cell is created and no frame is kept

### Requirement: Segments mapped from file pages
The kernel SHALL map an executable's and an interpreter's segments from
their files' pages: whole pages of a segment's file part are the file's,
the page where the file part ends and `.bss` begins is a copy with the
`.bss` zeroed, and pages of `.bss` alone are zeroed.

#### Scenario: Data followed by .bss
- **WHEN** a segment's file part ends inside a page where the file holds other bytes after it
- **THEN** the segment's `.bss` reads as zero in that page and the pages after it

### Requirement: File pages shared between cells
The kernel SHALL keep each file page it maps into a cell in one frame, by
the identity of its contents, and SHALL map that frame into every mapping
of the same bytes, in any cell, read-only; a write SHALL go to a private
copy. A page whose file changed SHALL NOT be mapped for the new contents.
Kept pages SHALL belong to the kernel, be taken like a cell's memory
(never from its reserve) and, if unmapped or held by no file, be dropped
with the kept executables before memory is refused to any cell.

#### Scenario: Two cells of one program
- **WHEN** two cells of the same dynamically linked program are started from the same files
- **THEN** their interpreter's code is the same frame in both, and each sees only its own writes to its data

#### Scenario: A file written after it was mapped
- **WHEN** a cell writes a file whose pages are kept and then maps it
- **THEN** the new mapping holds the new bytes and the earlier mappings the old ones

#### Scenario: Kept pages give way
- **WHEN** dynamically linked cells have ended and the kept executables and pages are dropped
- **THEN** free memory and the kernel heap are what they were before the cells started

### Requirement: Dynamically linked cells are plain data
A cell started from its files SHALL be suspended, resumed, forked and
frozen as a template like any other: a snapshot SHALL hold the contents of
every mapped page, so it resumes without the pages kept for the files, and
forks and cells from a template SHALL share every page copy-on-write.

#### Scenario: Suspend, resume, fork and template
- **WHEN** the dynamic `hello` has printed `ready`, is suspended, resumed from the snapshot's bytes and forked twice, and another is frozen at `ready` and two cells are spawned from it
- **THEN** every one of them finishes the program, printing its last line, and exits with 0

### Requirement: Large pages for executables
When the kernel loads an executable, every 2 MiB-aligned 2 MiB piece that
lies wholly inside one segment SHALL be mapped with a 2 MiB page when a
2 MiB frame is free, with the segment's permissions and contents; the rest
of each segment SHALL be mapped with 4 KiB pages, and nothing outside the
segments SHALL be mapped.

#### Scenario: A segment with whole 2 MiB pieces
- **WHEN** an executable whose code segment covers two whole 2 MiB-aligned pieces is loaded
- **THEN** those pieces are backed by 2 MiB pages, the rest of the segment by 4 KiB pages, and every byte reads as in the file

#### Scenario: No 2 MiB frame free
- **WHEN** no 2 MiB frame is free while such an executable is loaded
- **THEN** it is loaded with 4 KiB pages only and runs the same

### Requirement: No cell evicts the pages another maps
No cell SHALL make the kernel drop a kept file page that a mapping uses
and a file holds. When the kept pages reach their limit, the kernel SHALL
drop only pages no file holds any more, and a page it has no room for
SHALL be a copy of the mapping cell's own, counted in its memory quota.

#### Scenario: A full cache
- **WHEN** one cell maps a kept page and another maps more pages of its own files than the kept pages' limit
- **THEN** the first cell's page stays kept, and the pages past the limit are the second cell's own copies

#### Scenario: Memory pressure from another cell
- **WHEN** a cell maps a page of its file, and another cell takes memory until it is refused
- **THEN** the first cell's page stays kept
