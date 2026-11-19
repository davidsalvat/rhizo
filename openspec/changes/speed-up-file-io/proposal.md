# Proposal

## Why

Creating, writing and reading back 1 MiB of file data takes about 1 ms in a
cell on KVM (`cell.file_write_read_us`), against microseconds for every
other cell operation. Reading cell memory goes byte by byte (one 64-bit
load and a shift per byte), and a file write copies each page twice (cell
memory to a kernel buffer, then into a freshly zeroed block). Rule 7 asks
for the fastest correct option. The two file metrics are also not in the
`ci` baseline yet, so nothing stops them from regressing.

## What Changes

- Copies out of cell memory use the bulk `PhysicalMemory::read_bytes`
  (a `memcpy` through the direct map in the kernel), one translation per
  page, instead of a load per byte. This speeds up every system call that
  reads cell memory (`write` to the console as well as to files).
- A file write copies cell memory straight into the file block, and a read
  copies the block straight into cell memory: one copy instead of two.
- A file block's reference count lives apart from its data: a block was
  4112 bytes and took two contiguous pages of the kernel heap, so file data
  used twice its size (the 64 MiB bound really allowed 128 MiB). Now the data
  fills exactly one page.
- `cell.file_write_read_us` and `cell.fork_with_files_us` join the `ci`
  baseline, so the bench gate covers them.

### Out of scope

Mapping file blocks into cells (`mmap` of files), larger blocks.

## Capabilities

### Modified Capabilities

- `benchmarks`: the file metrics are gated by the `ci` baseline.
- `files`: file data takes its own size in memory.

## Impact

`crates/rhizo-cell` (memory copies), `crates/rhizo-fs` (block access for
in-place copies), `crates/rhizo-abi` (file I/O), `bench/baseline.json`. No
change in behavior: every existing test holds unchanged.
