# fork Specification

## Purpose
Defines forking a cell into identical copies that share memory until they
write to it, and templates: frozen cells used as the source of forks.

## Requirements

### Requirement: Fork
The kernel SHALL create `n` children of a live cell, each with the parent's
registers, memory, program break and mapping cursor, a new identifier and
its own address space.

#### Scenario: Children continue like the parent
- **WHEN** a cell is forked into children and all of them run to their end
- **THEN** each prints what the parent prints from the fork point on, except for values derived from its identifier

### Requirement: Copy-on-write isolation
After a fork, a write by any cell SHALL be visible to that cell only,
whether done by cell code or by a system call on its behalf.

#### Scenario: Own values
- **WHEN** a parent and its children write their identifiers into the same shared pages and read them back after yielding
- **THEN** each reads its own identifier

#### Scenario: System call into a shared page
- **WHEN** a child passes a shared page as the output buffer of a system call
- **THEN** the call fills a private copy and the parent's page is unchanged

### Requirement: Fork cost independent of memory size
Forking SHALL NOT copy pages or leaf page tables; it SHALL allocate only the
child's root table.

#### Scenario: Large cell
- **WHEN** a cell with 50 MiB of memory is forked
- **THEN** exactly one page-table frame is allocated

### Requirement: Templates
A frozen cell SHALL not run, and spawning from it SHALL fork it.

#### Scenario: Spawn from template
- **WHEN** a cell is spawned from a template of the `hello` program
- **THEN** it prints the same output as a cell spawned from the executable

### Requirement: Files are forked copy-on-write
A forked cell SHALL start with its parent's files, descriptors and working
directory; afterwards each SHALL see only its own changes, and a fork SHALL
NOT copy file contents.

#### Scenario: Diverging files
- **WHEN** a parent and its child each write different contents to the same file after a fork
- **THEN** each reads its own contents

#### Scenario: Template files
- **WHEN** cells are spawned from a template that has files
- **THEN** each starts with those files

### Requirement: Forks carry every thread
Forking a cell and spawning from a template SHALL copy every thread of the
cell (registers, FP/SIMD state, thread pointer, identifier, clear-tid
address and what it waits for). Unlike Linux `fork`, which copies only the
calling thread, the host forks a cell from outside, so no thread is the
caller, and dropping threads would leave the locks they hold locked.

#### Scenario: Threaded children
- **WHEN** a cell whose worker threads wait on a condition variable is forked, and each copy then signals its workers
- **THEN** in every copy all workers run, and each copy prints the same output

### Requirement: Templates keep no time
In a cell spawned from a template, the sleeps of the template's threads
SHALL be over and their futex waits with a timeout SHALL have timed out;
futex waits without a timeout SHALL still wait.

#### Scenario: Template of a threaded program
- **WHEN** cells are spawned from a template of a multi-threaded program frozen after its warm-up
- **THEN** each runs all its threads to the same answer as a cold run

### Requirement: Pipes are forked copy-on-write
A forked cell SHALL start with copies of its parent's pipes (with their
data), event counters and `epoll` instances; afterwards each SHALL see only
its own reads and writes.

#### Scenario: Diverging pipe
- **WHEN** a cell with unread bytes in a pipe is forked and parent and child each read the pipe
- **THEN** both read the same bytes

### Requirement: Copy-on-write needs no free 2 MiB block
A write to a 2 MiB page shared with another cell SHALL succeed while
4 KiB frames remain, even when no whole free 2 MiB block is left.

#### Scenario: Fragmented memory
- **WHEN** a forked cell writes to a shared 2 MiB page and memory has free 4 KiB frames but no free 2 MiB block
- **THEN** the write succeeds, the cell sees its own data, and the other cell does not see the write

### Requirement: Copy-on-write without a free large frame
A write to a 2 MiB page shared copy-on-write SHALL succeed whenever memory
holds 2 MiB of free frames, contiguous or not: with no free 2 MiB block,
the writer's copy SHALL be made of small pages.

#### Scenario: Fragmented memory
- **WHEN** a cell writes to a large page it shares with its parent and no 2 MiB block is free
- **THEN** the write succeeds in a private copy made of small pages, the parent's page is unchanged, and every frame is returned when both end
