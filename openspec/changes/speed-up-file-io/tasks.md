# Tasks

## 1. Copies

- [x] 1.1 `copy_from` with bulk `read_bytes` per page (D1); host tests of `rhizo-cell` and `rhizo-abi` unchanged and passing
- [x] 1.2 In-place file block access in `rhizo-fs` and one-copy file I/O in `rhizo-abi` (D2); host tests (partial counts and `EFAULT` across page boundaries, nothing written on a fault) and the property test pass

- [x] 1.3 One page per block (D2b); in-kernel test: holding a 16 MiB file costs less than 17 MiB

## 2. Benchmarks and gate

- [x] 2.1 `cell.file_write_read_us` measured faster in CI with no other metric worse; numbers in the pull request
- [x] 2.2 File metrics added to the `ci` baseline (D3); `cargo xtask ci` on both architectures
