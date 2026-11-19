# Design

## D1. Bulk copies out of cell memory

`copy_from` translates once per page and then reads the piece with
`PhysicalMemory::read_bytes`, as `copy_to` already writes with
`write_bytes`. In the kernel both are a `memcpy` through the direct map;
the host test double keeps its byte-wise default.

## D2. One copy between a cell and a file

`rhizo-fs` hands out the bytes of one file block for a piece of a file
(`Fs::write_with` / `Fs::read_with`, with a closure), doing the bookkeeping
itself: size, block allocation, the data bound, holes reading as zeros.
`rhizo-abi` fills or drains that slice straight from cell memory with the
existing pager-aware `memory::read`/`write`, so a page crosses once. Each
piece lies within one cell page and one block, so a fault in cell memory
happens before anything is copied; the piece is then dropped (a block
allocated for it is freed) and the call returns `EFAULT` or the count so far,
as before. `Fs::read`/`Fs::write` become loops over the same two functions.

| Option | Copies per 4 KiB written | Notes |
|---|---|---|
| **In place (chosen)** | 1 | closure API in `rhizo-fs` |
| Kernel buffer (before) | 2 | simplest |
| Map blocks into the cell | 0 | needs file `mmap`; out of scope |

## D2b. One page per block

`Arc<[u8; 4096]>` is 4112 bytes, above the heap's largest size class
(2048), so each block took two contiguous frames. `Arc<Box<[u8; 4096]>>`
puts the count in a 32-byte block and the data in exactly one frame. Reads
pay one more pointer hop, against half the memory and a single-frame
allocation per block.

## D3. Gate the file metrics

`bench/baseline.json` (`ci`) gains `cell.file_write_read_us` and
`cell.fork_with_files_us` per VMM, from this change's CI run, with the same
thresholds as the other timings.
