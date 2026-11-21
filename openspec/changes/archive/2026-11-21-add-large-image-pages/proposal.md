# Proposal

## Why

After `add-image-cache`, almost all of CPython's cold start (about 15.5 of
17.4 ms) is spent in user mode, running the interpreter. Its executable
code is 4.2 MB, mapped with 4 KiB pages: far more than the instruction TLB
covers, and under a VMM every TLB miss walks two levels of page tables.
2 MiB pages cover the same code with three entries. Anonymous mappings of
2 MiB or more already use them; executables never did.

## What Changes

- **Executables on 2 MiB pages**: the loader maps every 2 MiB-aligned
  2 MiB piece that lies wholly inside one segment with a 2 MiB page (when a
  2 MiB frame is free), and the rest with 4 KiB pages as before. Nothing is
  mapped that the segment does not cover, so permissions, the break and
  every small executable are unchanged.
- **CPython linked for it**: `xtask` links the static CPython with
  `-z max-page-size=0x200000`, which places its segments on 2 MiB
  boundaries: two of the three 2 MiB pieces of its 4.2 MB of code become
  whole pieces (one without relinking).
- **Measured, and kept only where it helps**: 2 MiB pages for the break
  were measured too and made the cold start slower; they are not part of
  the change (design D3).

### Out of scope

2 MiB pages for small anonymous mappings (CPython's 1 MiB arenas: a
`munmap` of half a 2 MiB page is refused, and splitting pages is not
supported), for the break (measured slower, D3), and for read-only data or
writable data smaller than 2 MiB (rounding them out would map memory the
executable does not describe, and copy 2 MiB per spawn for data).

## Capabilities

### Modified Capabilities

- `cells`: large pages for executables.

## Impact

`crates/rhizo-cell/src/elf.rs` (loader), `crates/rhizo-cell/src/lib.rs`,
`xtask/src/cpython.rs` (link flags, part of the build recipe, so CI
rebuilds CPython once). `workload.cpython_cold_us` drops by about 1.5 %;
no other metric changes (no other executable has a whole 2 MiB piece).
