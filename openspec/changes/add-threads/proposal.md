# Proposal

## Why

Real agent runtimes are multi-threaded: CPython starts threads for its
standard library and C extensions, Node runs a libuv worker pool, and
glibc's `pthread_create` is how both get them. Today a cell is a single
thread: `clone` does not exist, `futex` pretends nobody else is waiting,
and `gettid` is the cell's identifier. Those runtimes cannot run.

## What Changes

- **Threads inside a cell** (milestone H14), still on one CPU: `clone` and
  `clone3` with the flags `pthread_create` uses, a new stack and thread
  pointer per thread (`TPIDR_EL0`, the `FS` base), `set_tid_address`,
  `gettid` per thread, `exit` of one thread against `exit_group`, and on
  thread exit the clear-tid write and futex wake that `pthread_join`
  waits for.
- **Futexes that block**: `FUTEX_WAIT`, `FUTEX_WAKE`, `FUTEX_WAIT_BITSET`
  and `FUTEX_WAKE_BITSET` (private or not, timeouts on the monotonic or
  real-time clock). A waiting thread uses no CPU.
- **Scheduling**: the cell stays the unit the scheduler shares the CPU
  between; its threads take turns inside the cell's time slices, so
  creating threads does not take CPU from other cells. All threads of a
  cell share its address space and its tag (ASID/PCID): switching between
  them changes no translation.
- **Reserved mappings**: an anonymous `PROT_NONE` mapping reserves
  addresses that a later `mprotect` backs with zeroed memory, which is how
  glibc builds thread stacks with a guard page and per-thread malloc arenas.
- **State stays plain data** (rule 6): every thread (registers, FP/SIMD
  state, thread pointer, identifier, clear-tid address, what it waits for)
  is in the snapshot (format version 4) and in forks and templates. A fork
  of a multi-threaded cell copies all of its threads.
- **Tests**: host tests of the thread table, the futex calls, `clone` and
  the snapshot format; a `no_std` Rust cell (`threads`) for raw `clone`,
  futexes, thread pointers and register preservation across thread
  switches; a static glibc program (`libc_threads`) with `pthread_create`,
  mutexes, condition variables and `pthread_join`, also checked natively
  on Linux; in-kernel tests for suspend/resume and fork of threaded cells.
- **Benchmarks**: `cell.thread_create_us` (create and join a thread) and
  `cell.futex_wake_ns` (one futex hand-off between two threads).

### Out of scope

Several CPUs (the design keeps the thread table usable when threads of a
cell run on several), signals and `tgkill`, robust and priority-inheritance
futexes, `FUTEX_REQUEUE` and `FUTEX_WAKE_OP` (glibc 2.25 and later use
neither for its mutexes and condition variables), processes inside a cell
(`fork`, `vfork`, `clone` without `CLONE_VM`), and demand-zero memory
(a reserved stack is backed eagerly when `mprotect` opens it).

## Capabilities

### New Capabilities

- `threads`: threads of a cell, their identifiers, creation and exit,
  futexes.

### Modified Capabilities

- `cells`: per-thread state is plain data; FP/SIMD state is per thread.
- `linux-abi`: `futex` is no longer single-threaded; reserved mappings.
- `scheduling`: threads share their cell's time; blocked threads use no CPU.
- `snapshots`: snapshots hold every thread.
- `fork`: forks and templates carry every thread.
- `workloads`: a threaded glibc program.
- `benchmarks`: thread metrics.

## Impact

`crates/rhizo-cell` (thread table), `crates/rhizo-abi` (`clone`, `clone3`,
`futex`, `exit`, `gettid`, `set_tid_address`, `mprotect`),
`crates/rhizo-snapshot` (format version 4), `kernel/src/cell.rs` (thread
switches inside a slice, FP ownership per thread),
`kernel/src/arch/x86_64/user.rs` (`SYSRETQ` only for a thread that entered
through `SYSCALL`), `cells/` (two programs), `kernel/src/tests.rs`,
`xtask/src/bench.rs`, `bench/README.md`, `AGENTS.md` (code map).
