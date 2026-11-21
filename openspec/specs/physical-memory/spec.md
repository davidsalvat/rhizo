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

### Requirement: Freed frames are reused first
The frame allocator SHALL hand out frames freed below its search point before
frames that were never handed out.

#### Scenario: Reuse after free
- **WHEN** frames are freed below the most recently handed-out frame
- **THEN** the next allocation returns the lowest of them

### Requirement: Freeing a run costs per bitmap word
Freeing N contiguous frames SHALL check for a double free and mark the
frames free a 64-frame bitmap word at a time, not frame by frame, and
freeing one frame SHALL test and set its single bit. A double free inside
a run SHALL still panic, naming the lowest frame of the run that is
already free.

#### Scenario: Double free inside a run
- **WHEN** a run of 80 frames spanning three bitmap words is freed while two of its frames are already free
- **THEN** the kernel panics with a message naming the lower of those two frames

#### Scenario: Freeing a large page
- **WHEN** a cell writing a 50 MiB template's pages copies them as 2 MiB pages and exits
- **THEN** `cell.fork_touch_insns` is lower than with a frame-by-frame check

### Requirement: Pages zeroed and copied by the CPU's instructions
The kernel SHALL zero whole 4 KiB frames, and copy whole numbers of 4 KiB
pages, with the page primitives of its `arch` interface: on AArch64
`DC ZVA` where `DCZID_EL0` permits it and `LDP`/`STP` of general-register
pairs, on x86_64 `rep stosq` and `rep movsq`. The primitives SHALL use no
SIMD or floating-point register and SHALL give the same bytes as a byte
loop for any alignment of a copy's source and destination.

#### Scenario: Zeroed frame
- **WHEN** a frame filled with random bytes is zeroed through the direct map
- **THEN** every byte of it reads zero and the frames around it are unchanged

#### Scenario: Copies at any alignment
- **WHEN** whole pages of random bytes are copied frame to frame, and to and from kernel buffers at every offset modulo 16
- **THEN** the destination holds exactly the source bytes

### Requirement: Small copies keep memcpy
Copies of a length that is not a whole number of pages (user memory,
records) SHALL keep the compiler's `memcpy`, with no added test on their
path.

#### Scenario: Pipe round trip
- **WHEN** the pipe round-trip benchmark runs under QEMU `-icount`
- **THEN** its instruction count stays within the gate of the baseline
