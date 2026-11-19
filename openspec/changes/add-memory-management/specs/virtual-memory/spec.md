# Spec Delta

## Purpose

Defines the kernel's virtual address-space layout, the protections applied to
kernel memory, and the page-table operations used to build address spaces.

## ADDED Requirements

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
