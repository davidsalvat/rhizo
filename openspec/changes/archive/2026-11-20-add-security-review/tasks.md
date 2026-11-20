# Tasks

## 1. Review

- [x] 1.1 Threat model, trust boundary and every system call reviewed in `docs/security.md`; verify each `Sys` variant in `crates/rhizo-abi/src/lib.rs` has a row
- [x] 1.2 Findings confirmed before fixing (host tests failing, or the in-kernel test hanging or the escalation reproduced under QEMU); verify each finding in `docs/security.md` names how it was confirmed

## 2. Fixes

- [x] 2.1 `USER_END` checked in user-memory copies, `space` and the ELF loader and snapshots (D1); verify `cargo test -p rhizo-cell` and `-p rhizo-snapshot`
- [x] 2.2 Copy-on-write walks stay in the lower half (D1); verify `cargo test -p rhizo-mm kernel_half`
- [x] 2.3 `mprotect` limited to the cell, `arch_prctl` `EPERM`, one report for large unknown numbers; verify `cargo test -p rhizo-abi hostile`
- [x] 2.4 Hole skipping in `unmap`, `mprotect` stops at the first missing page (D2); verify `cargo test -p rhizo-mm holes` and `-p rhizo-abi huge_empty`
- [x] 2.5 File limit and memory check for names, `getdents64` bound (D3), captured output bound (D4), refuse before zeroing (D5); verify `cargo test -p rhizo-fs limited` and the in-kernel `names_cannot_fill_kernel_memory`
- [x] 2.6 Snapshot register checks (D6); verify `cargo test -p rhizo-snapshot`
- [x] 2.7 `mprotect` makes a shared frame writable only copy-on-write (D1b); verify `cargo test -p rhizo-cell shared_page` and the in-kernel `shared_read_only_pages_stay_shared_after_mprotect`

## 3. Hostile tests

- [x] 3.1 Host property tests in `rhizo-abi/src/hostile.rs` (D7); verify `cargo xtask check`
- [x] 3.2 Hostile cells and in-kernel tests on both architectures; verify `cargo xtask test --only ktest` and `cargo xtask --arch x86_64 test --only ktest`
- [x] 3.3 Everything CI runs; verify `cargo xtask ci` on both architectures and CI green
