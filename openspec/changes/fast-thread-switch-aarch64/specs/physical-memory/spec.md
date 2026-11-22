## MODIFIED Requirements

### Requirement: Small copies keep memcpy
Copies of a length that is not a whole number of pages (user memory,
records) SHALL keep going through `memcpy`, with no added test on their
path: the kernel's own on AArch64, `compiler_builtins`' on x86_64.

#### Scenario: Pipe round trip
- **WHEN** the pipe round-trip benchmark runs under QEMU `-icount`
- **THEN** its instruction count stays within the gate of the baseline

## ADDED Requirements

### Requirement: The kernel's memory routines on AArch64
On AArch64 the kernel SHALL define its own `memcpy`, `memmove`, `memset`
and `memcmp` in its `arch` code, moving 64 bytes per loop with `LDP`/`STP`
of general-register pairs and shorter lengths without a byte loop. They
SHALL use no SIMD or floating-point register, SHALL give the bytes and
the sign of comparison of a byte loop for any length and alignment, and
`memmove` SHALL copy overlapping ranges as if through a buffer.

#### Scenario: Routines match a byte model
- **WHEN** the in-kernel tests copy, move, fill and compare every length from 0 to 160 at every pair of offsets modulo 16, overlapping ranges at every distance up to 72 bytes in both directions, and random lengths and offsets up to 4,096
- **THEN** every result equals the byte model's and no byte around the destination changes

#### Scenario: Faster copies on AArch64
- **WHEN** the AArch64 cell benchmarks run under `-icount`
- **THEN** `cell.suspend_insns` and `cell.snapshot_export_insns` are lower than with `compiler_builtins`' routines
