# physical-memory Specification

## Purpose
Defines how the kernel discovers usable physical memory, keeps reserved memory
safe, hands out and takes back 4 KiB frames, and reports memory at boot.

## Requirements

### Requirement: Usable memory comes from the platform memory map
The kernel SHALL build its set of usable physical memory only from regions the
boot memory map marks as RAM, below 4 GiB. If the platform provides no memory
map, the kernel SHALL stop with a panic that says so.

#### Scenario: RAM regions become usable
- **WHEN** the memory map lists a RAM region and a reserved region
- **THEN** frames inside the RAM region can be allocated
- **AND** no frame inside the reserved region is ever allocated

#### Scenario: Missing memory map
- **WHEN** the boot information carries no memory map
- **THEN** the kernel panics with a message saying the memory map is missing

### Requirement: Memory in use at boot is never handed out
The allocator SHALL never return a frame that overlaps the first 1 MiB of
physical memory, the kernel image, or the boot information structures.

#### Scenario: Kernel image is protected
- **WHEN** every free frame is allocated
- **THEN** none of the returned frames overlaps the kernel image, the first 1 MiB or the boot information

### Requirement: Frames are unique and aligned
Each allocated frame SHALL be 4 KiB in size and 4 KiB aligned, and a frame
SHALL NOT be returned again until it has been freed.

#### Scenario: Many allocations
- **WHEN** frames are allocated repeatedly without freeing
- **THEN** every returned address is a multiple of 4096 and no address repeats

#### Scenario: Freed frames are reusable
- **WHEN** a frame is freed after allocating every free frame
- **THEN** the next allocation succeeds and returns that frame

### Requirement: Contiguous allocation
The allocator SHALL allocate a run of N physically contiguous frames when such
a run is free, and SHALL report failure when no such run exists.

#### Scenario: Contiguous run
- **WHEN** 4 contiguous frames are requested and a free run of at least 4 exists
- **THEN** the returned address starts a run of 4 frames that are all marked in use

### Requirement: Exhaustion and misuse are detected
When no frame is free, allocation SHALL report failure without panicking.
Freeing a frame that is not allocated, not aligned or not managed SHALL panic.

#### Scenario: Out of frames
- **WHEN** a frame is requested and none is free
- **THEN** the allocator reports failure and the kernel keeps running

#### Scenario: Double free
- **WHEN** the same frame is freed twice
- **THEN** the kernel panics with a message identifying the frame

### Requirement: Memory summary at boot
During boot the kernel SHALL print one line with the total usable memory and
the memory still free after boot setup, both in MiB.

#### Scenario: Memory line
- **WHEN** the kernel boots with 128 MiB of RAM
- **THEN** the serial console shows a line starting with `memory:` that includes the usable and free amounts in MiB
