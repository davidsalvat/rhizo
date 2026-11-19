# Tasks

## 1. Floating point

- [x] 1.1 `FpState` in `Registers`, initial state per architecture, snapshot format version 2; host tests
- [x] 1.2 FP/SIMD enabled for cells, save/load on both architectures, lazy ownership in the host; tests: two FP cells, FP across suspend/resume

## 2. Linux ABI

- [x] 2.1 `mprotect` (via `space::protect`), limits, `fstat`/`newfstatat`, `readlink(at)`, ids, `sigaltstack`, `madvise`, `futex`, `rseq` declined; host tests
- [x] 2.2 Lazy resume: page in before `mprotect`, discard pending pages on `munmap` and `brk` shrink

## 3. Real programs

- [x] 3.1 `cells/c/` built statically by `xtask` with the system gcc/glibc, embedded with the Rust cells; differential check
- [x] 3.2 In-kernel tests: glibc programs, warm template of a real program
- [x] 3.3 `workload.cold_us` and `workload.warm_us` in `bench-cell` and `xtask bench`

## 4. Documentation

- [x] 4.1 README roadmap (H6), AGENTS.md (CI jobs build C cells with their native gcc; cross gcc only for local cross builds), `cargo xtask ci` on both architectures
