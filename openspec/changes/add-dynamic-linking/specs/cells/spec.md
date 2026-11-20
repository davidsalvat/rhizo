# Spec Delta

## ADDED Requirements

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
The kept pages SHALL belong to the kernel, be taken like a cell's memory
(never from the kernel's reserve), and be dropped with the kept executables
before memory is refused to any cell.

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
