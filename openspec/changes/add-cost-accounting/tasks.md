# Tasks

## 1. Libraries

- [x] 1.1 `rhizo-prof`: `Part`, `Account` (exclusive and inclusive time, counts, merge), `Calls`, `Breakdown` lines (D2, D3); host and property tests; verify `cargo test -p rhizo-prof`
- [x] 1.2 `SpinLock` contention counters and `with_waiting`, the free path unchanged (D4); host test with two threads; verify `cargo test -p rhizo-core sync`
- [x] 1.3 `PhysicalMemory::copy`, used by copy-on-write (D7); host test; verify `cargo test -p rhizo-hal -p rhizo-mm`
- [x] 1.4 `rhizo_abi::Arch::{call_number, call_name, is_socket_call}`; host test; verify `cargo test -p rhizo-abi call_numbers`

## 2. Kernel

- [x] 2.1 `arch::pmu` on both architectures (D5); in-kernel test `performance_counters_count_where_present`; verify `cargo xtask test --only ktest` on both architectures
- [x] 2.2 `prof` module, `profile` feature (in `ktest`), hooks (D1, D2); in-kernel test `cost_accounting_adds_up_over_a_cell`; verify `cargo xtask test --only ktest` on both architectures
- [x] 2.3 `bench-cell`: `cell.syscall_ns` (`yielder getpid`), PMU passes with `bench_clock` (D5); `profile` build: `cow_fault_ns`, breakdowns, `parallel` breakdown and lock waits; verify a run under `-icount` matches the kernel without hooks (D1)

## 3. xtask and documentation

- [x] 3.1 `--accel icount`, `profile.rs` (counted figures, PMU passes, breakdowns, the icount run), `--no-icount`, `thresholds_pct.instructions` and ungated prefixes; unit tests; verify `cargo xtask --arch x86_64 bench --runs 1 --vmm qemu-microvm --no-reference --accel tcg`
- [x] 3.2 `bench/README.md`, `AGENTS.md` (crate row), `xtask/src/deps.rs`; `cargo xtask ci` on both architectures
- [x] 3.3 CI green; note which VMMs give the guest a PMU (`rhizo-pmu` lines); add the `-icount` counts and the new timings to the `ci` baseline from that run's results
