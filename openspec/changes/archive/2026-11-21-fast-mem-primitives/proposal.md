# Proposal

## Why

Every page the kernel zeroes or copies goes through `memory::DirectMap`,
which calls `core::ptr::write_bytes` and `copy_nonoverlapping`, that is
`memset` and `memcpy` from `compiler_builtins`. On AArch64 those are
generic loops of 8-byte stores: about 2,050 instructions to copy a 4 KiB
page whose source and destination are aligned alike, about 1,540 to zero
one, and a byte-by-byte loop of about 22 instructions per 8 bytes when the
two are not aligned alike (a page copied into a heap buffer at an odd
offset). Copy-on-write faults, fork's child writing its pages, eager
resume, suspend, page zeroing for fresh memory and for write-or-execute
pages all pay it: the `fork_touch` breakdown spends 26.2 million of its
26.7 million instructions copying. On x86_64 `compiler_builtins` already
uses `rep movsq`/`rep stosq`, wrapped in two `rep movsb` for the head and
the tail.

Snapshot files (change `snapshots-on-disk`) carry a CRC-32C computed by a
slicing-by-8 table: about 3 instructions per byte, twice per round trip of
a file of tens of MiB, while both architectures have an instruction that
does 8 bytes at a time (SSE4.2 `crc32`, ARMv8 `CRC32CX`).

## What Changes

- **Page primitives in `arch`.** A new arch module `mem` with
  `zero_page(dst)` (4 KiB) and `copy_pages(dst, src, n)` (n times 4 KiB):
  - AArch64: `DC ZVA` over the page when `DCZID_EL0` allows it (block
    size read from `BS`), else 16-byte `STP` of the zero register;
    the copy moves 128 bytes per loop with `LDP`/`STP` of eight general
    registers. No SIMD register: the kernel stays soft-float, and a
    cell's vector registers are never touched.
  - x86_64: `rep stosq` and `rep movsq` of 512 quadwords, with no head or
    tail (fast strings on every x86_64 CPU; general registers only).
- **Wired through `PhysicalMemory`.** `DirectMap::zero_frame` uses
  `zero_page`; `copy` uses `copy_pages` for a whole number of pages. The
  trait gains `read_pages` and `write_pages` (whole pages, any
  alignment; defaults `read_bytes` and `write_bytes`), which
  `rhizo-snapshot` uses for page data and `DirectMap` implements with
  `copy_pages`; `read_bytes` and `write_bytes` keep `memcpy` for small
  copies. `rhizo-mm`, `rhizo-cell` and `rhizo-abi` do not change: they
  already zero and copy through the trait.
- **CRC-32C by the CPU.** `rhizo_snapshot::crc32c` (the table) becomes
  public, and `Snapshot::export` and `Snapshot::from_bytes` take the
  checksum function (`Crc32c`). The kernel passes `arch::mem::crc32c`:
  `CRC32CX`/`CRC32CB` where `ID_AA64ISAR0_EL1` lists CRC32, SSE4.2
  `crc32` where `CPUID` lists it, the table otherwise. The bytes of a
  snapshot file do not change.
- **Benchmarks.** `cell.snapshot_export_us` and `cell.snapshot_import_us`:
  the 8 MiB `ballast` snapshot exported to bytes with its checksum, and
  those bytes checked and decoded, in the cell benchmark (and so counted
  under `-icount`). The copy-on-write, fork, suspend and resume figures
  and the `copy` breakdowns show the page primitives.

### Out of scope

- `rep movsb`/`rep stosb` (ERMS, FSRM) on x86_64: measured, they count
  4,096 iterations per page under `-icount` instead of 512 (design D2),
  and the gain on 4 KiB-aligned pages is not shown.
- Overriding `memcpy`/`memset` for the whole kernel (file blocks of
  `rhizo-fs`, heap clones): a separate change with its own measurements.
- A pre-zeroed frame pool and per-CPU frame caches (another change of the
  same wave; it can call `arch::mem::zero_page`).
- SIMD copies (would need the kernel to save a cell's vector registers).

## Capabilities

- `physical-memory`: pages zeroed and copied with the CPU's instructions.
- `snapshots`: the checksum of exported snapshots computed by the CPU.
- `benchmarks`: snapshot export and import metrics.

## Impact

- `kernel/src/arch/{aarch64,x86_64}/mem.rs` (new), `kernel/src/memory.rs`,
  `kernel/src/control.rs`, `kernel/src/cell.rs` (benchmark),
  `kernel/src/tests.rs`.
- `crates/rhizo-hal`: `PhysicalMemory::{read_pages, write_pages}`.
- `crates/rhizo-snapshot`: `export` and `from_bytes` take a `Crc32c`;
  `crc32c` is public.
- `xtask`: the two new metrics; `bench/README.md`.
