# Tasks

## 1. Profile

- [x] 1.1 Find where pages are zeroed and copied, read the code the compiler emits, and count the `before` figures under `-icount` on both architectures (D1); verify `cargo xtask bench --icount-only` and `cargo xtask --arch x86_64 bench --icount-only` on the parent commit

## 2. Page primitives

- [x] 2.1 `arch::mem::{zero_page, copy_pages}` on AArch64 (`DC ZVA` with the `STP` fallback, `LDP`/`STP` of x registers) and on x86_64 (`rep stosq`, `rep movsq`) (D2, D3); verify `cargo xtask check`
- [x] 2.2 `DirectMap` zeroes frames and copies whole pages with them, `memcpy` for the rest (D4); verify the in-kernel test `page_zero_and_copy_match_byte_loops` with `cargo xtask test --only ktest` on both architectures

## 3. Checksum

- [x] 3.1 `rhizo_snapshot::crc32c` public, `Crc32c` passed to `export` and `from_bytes` (D5); verify `cargo test -p rhizo-snapshot checksum`
- [x] 3.2 `arch::mem::crc32c` with `CRC32C*` and SSE4.2 `crc32`, the table as fallback, passed by the control kernel and the tests (D5); verify the in-kernel test `hardware_checksum_matches_the_table` on both architectures

## 4. Benchmarks and gates

- [x] 4.1 `cell.snapshot_export_us` and `cell.snapshot_import_us` (D6); verify `cargo test -p xtask snapshot_line`
- [x] 4.2 `after` figures under `-icount` on both architectures, every gate passing without baseline edits (D7); verify `cargo xtask bench --icount-only --env ci --check` on both architectures
- [x] 4.3 `cargo xtask check`, `cargo xtask test` on both architectures; verify they pass
