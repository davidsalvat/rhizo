# Tasks

## 1. Libraries

- [x] 1.1 `rhizo_sched::Gangs`: start, offer, join, leave, next wake (D1, D5); unit tests and property tests of random interleavings of CPUs against a model; verify `cargo test -p rhizo-sched`
- [x] 1.2 `Threads::pick`, `make_current`, `get` in `rhizo-cell` (D4, D5); unit and property tests (round robin by identifier, busy threads skipped, deadlines expire); verify `cargo test -p rhizo-cell`

## 2. Kernel

- [ ] 2.1 Gangs in `kernel/src/cell.rs`: going shared, joining, the cell lock, per-CPU registers, eager FP put-back, offering threads and waking idle CPUs, leaving, ending on every CPU (D2–D6); single-CPU path unchanged; verify `cargo xtask test` and `cargo xtask test --only ktest-smp` on both architectures
- [ ] 2.2 x86_64 shootdowns: per-CPU user-mode words, hold flag, interrupts, clean set (D7); none on AArch64; verify `cargo xtask --arch x86_64 test --only ktest-smp`
- [x] 2.3 Cells `parsum` (C, pthreads) and `shootdown` (Rust), in `cells/expected.rs`; verify `cargo xtask test --only differential`
- [ ] 2.4 In-kernel tests: parallel speed-up and result of `parsum`, unmap under a reader, suspend, resume and fork of a cell on several CPUs, futex ping-pong between CPUs; verify `cargo xtask test` and `ktest-smp` on both architectures, `ktest-smp` three times in a row

## 3. Benchmarks and documentation

- [x] 3.1 `cell.thread_scaling_pct` and `cell.shootdown_us` in the `rhizo-smp` line, parsed by `xtask` with a unit test, `bench/README.md` (D8); verify `cargo xtask --arch x86_64 bench --runs 1 --vmm qemu-microvm --no-reference` locally
- [ ] 3.2 `AGENTS.md`, `README.md` feature list; `cargo xtask ci` on both architectures; CI green with the bench gate
