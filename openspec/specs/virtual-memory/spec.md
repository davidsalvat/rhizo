# virtual-memory Specification

## Purpose
Defines the kernel's virtual address-space layout, the protections applied to
kernel memory, and the page-table operations used to build address spaces.

## Requirements

### Requirement: Kernel runs in the higher half
After boot setup the kernel SHALL execute from the top 2 GiB of the virtual
address space, and the lower half of the address space (user range, below
`0x0000_8000_0000_0000`) SHALL contain no mappings.

#### Scenario: Lower half is empty
- **WHEN** boot setup has finished
- **THEN** translating address `0x0` and the kernel's physical load address returns no mapping

#### Scenario: Null pointer access
- **WHEN** kernel code reads or writes address `0x0`
- **THEN** a page fault is raised and reported

### Requirement: All RAM is reachable through a direct map
The kernel SHALL map every usable physical address `p` at virtual address
`0xffff_8000_0000_0000 + p`, readable and writable, never executable.

#### Scenario: Direct map access
- **WHEN** kernel code writes a value through the direct-map address of an allocated frame
- **THEN** reading that frame through any other mapping of it returns the same value

### Requirement: W^X for kernel memory
No kernel mapping SHALL be both writable and executable. Kernel code SHALL be
mapped read-only and executable, read-only data read-only and non-executable,
and writable data non-executable.

#### Scenario: Code is not writable
- **WHEN** the mapping of a kernel function's address is inspected
- **THEN** it is executable and not writable

#### Scenario: Data is not executable
- **WHEN** the mapping of a kernel writable static is inspected
- **THEN** it is writable and not executable

### Requirement: Stack overflow is caught by a guard page
The page directly below the kernel stack SHALL be unmapped, so that overflowing
the stack raises an exception instead of corrupting adjacent memory.

#### Scenario: Unbounded recursion
- **WHEN** kernel code recurses until the stack is exhausted
- **THEN** the overflow is reported as a CPU exception and the VM exits with failure

### Requirement: Page mapping operations
The kernel SHALL be able to map a 4 KiB virtual page to a physical frame with
chosen writable, executable and user-accessible permissions, translate a
virtual address to its frame and permissions, and unmap a page.

#### Scenario: Map, use and unmap
- **WHEN** a frame is mapped at an unused virtual address, written through it and unmapped
- **THEN** the write is visible through the direct map
- **AND** after unmapping, translation of that address returns no mapping

#### Scenario: Double mapping is rejected
- **WHEN** a virtual page that is already mapped is mapped again
- **THEN** the operation fails with an already-mapped error and the existing mapping is unchanged

#### Scenario: Unmapping flushes stale translations
- **WHEN** a page is unmapped
- **THEN** the processor's cached translation for that page is invalidated

#### Scenario: Non-canonical address
- **WHEN** a mapping is requested at a non-canonical virtual address
- **THEN** the operation fails without modifying any page table

### Requirement: Shared frames and tables
A frame or page table SHALL be returned to the allocator only when the last
cell referring to it releases it.

#### Scenario: Parent ends first
- **WHEN** a parent ends while its children still run
- **THEN** the children keep their memory, and after all end every frame is free again

### Requirement: Effective permissions
A page SHALL be writable only if its entry and every table entry above it
allow writing, for cell code and for kernel accesses on a cell's behalf.

#### Scenario: Read-only table
- **WHEN** a table entry is read-only and the page below it is writable
- **THEN** translation reports the page read-only

### Requirement: Write-or-execute pages
A cell page asked writable and executable SHALL carry a software bit in its
page-table entry and SHALL be, at every moment, either writing (writable or
copy-on-write, not executable) or running (read-only, executable); no
page-table entry of a cell SHALL allow both writing and instruction
fetches.

#### Scenario: Never both
- **WHEN** any sequence of mapping, protecting, forking, cloning, writing and fetching is applied to write-or-execute pages, in either entry format
- **THEN** after every step no leaf entry allows both writing and execution

#### Scenario: The bit in both formats
- **WHEN** a write-or-execute leaf is encoded in the x86_64 or AArch64 format and decoded
- **THEN** the flags come back with the write-or-execute bit, which the hardware ignores

### Requirement: Changing between writing and running
An instruction fetch from a writing page SHALL make it running, and a
write to a running page SHALL make it writing. Each change SHALL flush the
page's old translation from every CPU that may hold it before the access
is retried, and the change to running SHALL make the page's contents
visible to instruction fetches on every CPU.

#### Scenario: Code written then run
- **WHEN** a cell writes an instruction sequence into a write-or-execute page and jumps to it
- **THEN** the sequence runs and returns, and the page's entry is read-only and executable

#### Scenario: Code rewritten
- **WHEN** the cell then writes a different sequence into the same page and jumps to it
- **THEN** the new sequence runs, never the old one

#### Scenario: Written by one thread, run by another
- **WHEN** one thread of a cell writes code into a page that another thread, on another CPU, runs
- **THEN** both make progress and the running thread runs only code completely written
