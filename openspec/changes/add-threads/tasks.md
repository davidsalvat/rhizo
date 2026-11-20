# Tasks

## 1. Thread table

- [x] 1.1 `rhizo_cell::threads` (D1, D2, D3): `Thread`, `Wait`, `Threads` (current, spawn, exit, block, wake, next, wake_at, expire_timers); `Cell` carries it; host unit and property tests; verify `cargo test -p rhizo-cell`

## 2. System calls

- [x] 2.1 `clone`, `clone3` (D5), `gettid`, `set_tid_address`, `exit` of one thread with clear-tid wake, `exit_group`; `Services::save_fp`; host tests; verify `cargo test -p rhizo-abi`
- [x] 2.2 `futex` wait/wake and bitset variants with timeouts (D3); sleeps recorded in the thread table; host tests; verify `cargo test -p rhizo-abi`
- [x] 2.3 Reserved mappings: `mprotect` backs holes below the mapping cursor (D6); host test; verify `cargo test -p rhizo-abi`

## 3. Snapshots

- [x] 3.1 Format version 4 with every thread (D7), capture and restore of waits as time left, validation; host tests; verify `cargo test -p rhizo-snapshot`

## 4. Kernel

- [x] 4.1 Host: threads switch inside a slice, cells park when all threads wait, FP ownership per thread, fork/freeze/suspend/resume with threads, templates expire time (D2, D8); verify `cargo xtask test` on both architectures
- [x] 4.2 x86_64 `SYSRETQ` only when RCX and R11 match (D9); verify `cargo xtask --arch x86_64 test`

## 5. Test programs and in-kernel tests

- [x] 5.1 `threads` Rust cell: clone, gettid, thread pointers, futex wait/wake/timeouts/bitsets, join through clear-tid, registers across preemption; in `cells/expected.rs`; verify the differential check and `cargo xtask test --only ktest`
- [x] 5.2 `libc_threads` C cell: pthreads, mutexes, condition variables, join, default stacks; in `cells/expected.rs`; verify the differential check and `cargo xtask test --only ktest`
- [x] 5.3 In-kernel tests: threaded cells next to others (spinning thread, thread count does not buy time, parked cells), suspend/resume after every step, fork and template of threaded cells, memory returned; verify `cargo xtask test` on both architectures

## 6. Benchmarks and documentation

- [ ] 6.1 `cell.thread_create_us` and `cell.futex_wake_ns` (D10): `bench` mode of `threads`, `bench-cell` line, parsing in `xtask/src/bench.rs` with a unit test, `bench/README.md`; verify `cargo xtask bench --runs 1` locally and the CI bench gate
- [ ] 6.2 `AGENTS.md` code map; `cargo xtask ci` on both architectures; CI green
