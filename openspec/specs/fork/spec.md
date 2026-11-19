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
