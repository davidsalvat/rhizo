# Spec Delta

## ADDED Requirements

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
