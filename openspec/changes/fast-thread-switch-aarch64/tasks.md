# Tasks

## 1. Profile

- [x] 1.1 Read what `memcpy`, `memmove`, `memset` and `memcmp` resolve to in the AArch64 kernel and count the `before` figures under `-icount` on both architectures (D1, D4); verify `cargo xtask --arch aarch64 bench --icount-only` and `cargo xtask --arch x86_64 bench --icount-only` on the parent commit

## 2. Memory routines

- [x] 2.1 `memcpy`, `memmove`, `memset` and `memcmp` in `kernel/src/arch/aarch64/mem.rs`, general registers only (D1); verify `cargo xtask check`
- [x] 2.2 In-kernel test `memory_routines_match_a_byte_model` against a byte model, lengths and offsets up to 4,096, overlapping moves both ways (D1); verify `cargo xtask test --only ktest` on both architectures, and that an always-forward `memmove` fails it

## 3. Thread switch

- [x] 3.1 `Threads::next` trades the two records instead of moving them out and back, same turn order (D2); verify `cargo test -p rhizo-cell threads`

## 4. Benchmarks and gates

- [x] 4.1 `after` figures under `-icount` on both architectures, AArch64 `cell.loopback_rtt_insns` at or below 9,206, x86_64 `cell.syscall_insns` and `cell.pipe_rtt_insns` unchanged, no baseline edits (D4); verify `cargo xtask bench --icount-only --check` on both architectures
- [x] 4.2 `cargo xtask check`, `cargo xtask ci` on both architectures; verify they pass
