# Design

## D1. Whole 2 MiB pieces of a segment get 2 MiB pages

`Image::load` walks each segment; where the next page is 2 MiB-aligned and
the segment (rounded to 4 KiB pages, as before) covers the whole 2 MiB from
there, it asks for a 2 MiB frame and maps it with one entry; otherwise, or
when no 2 MiB frame is free, a 4 KiB page as before. The file's bytes are
copied to where they belong in either kind of page and the rest is zero.
The loader now takes `Frames` (it needs `alloc_large`) and gives back a
frame whose mapping fails.

Only the kept space of an executable (`add-image-cache`) is loaded this
way; its copies share read-only 2 MiB pages like 4 KiB ones (one reference
each, no table under them), so they cost no memory and make the copy
cheaper (one entry instead of 512). A writable whole piece would be copied
2 MiB at a time per spawn, as any writable page is; no executable built
here has one.

Rejected: rounding segments out to 2 MiB. It would cover CPython's
read-only data (1.9 MB) and the rest of its code, but maps memory the
executable does not describe (reads past a segment would succeed), moves
the break, and for writable data copies 2 MiB per spawn and per cell.

## D2. CPython linked with 2 MiB segment alignment

With the default 4 KiB alignment, CPython's code spans
`0x401000..0x8298ad`: one whole 2 MiB piece. Linked with
`-z max-page-size=0x200000` the linker starts every segment at a 2 MiB
boundary (`0x600000..0xa288ad`): two whole pieces, 4 of its 4.2 MB. The
file grows by the padding (7.2 → 13 MB, in the `bench-cell` kernel and
the CI cache, not in what a cell maps), and Linux runs the same binary in
the differential check and the comparison VM. The flags are part of the
build recipe, so a stale build is never reused.

## D3. What was measured

A diagnostic build (never merged) ran, inside each benchmark boot, the
CPython cold start interleaved in four modes, six times each, the image
reloaded for each mode: 4 KiB pages only; 2 MiB pages for whole pieces of
the image (D1); 2 MiB pages for the break past its first 2 MiB boundary
(mapped and zeroed 2 MiB at a time); both. Paired per boot, over 20
Firecracker boots on two CI runs (KVM, x86_64), against 4 KiB pages only:

| Mode | Fast runner (17.3 ms) | Slow runner (24.4 ms) |
|---|---|---|
| Image on 2 MiB pages | −1.6 % (every boot between −3.3 % and −0.7 %) | −0.9 % (−4.0 % to +2.7 %) |
| Break on 2 MiB pages | +0.5 % (−0.1 % to +1.1 %) | +1.9 % (−3.0 % to +6.6 %) |
| Both | −0.7 % | 0.0 % |

The image's pages help a little but consistently; the break's cost more
(each 2 MiB page is zeroed when the break crosses into it, most of it
never used) than its TLB entries save. Only D1 and D2 ship. The figures
also say that TLB misses are not where CPython's user time goes: the cold
start is dominated by the interpreter's own work, so large pages cannot
remove more than a few percent of it.

The CI benchmark run of the change itself (unpaired, against the
`add-image-cache` run before it) puts `workload.cpython_cold_us` between
−1.2 % and +0.6 % across the four VMMs: inside the runners' noise, which
is why the paired figures above decide. No other metric moves beyond the
noise `main` shows between runs (the fork figures, which no executable
here affects, swing by tens of percent between any two runs).
