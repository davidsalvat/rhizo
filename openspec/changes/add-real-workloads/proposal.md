# Proposal

## Why

So far cells run tiny `no_std` programs written for Rhizo. The thesis is
about real Linux programs: a cell must run what an agent would run, with a
real libc, floating point and SIMD, unchanged, and behave exactly as on
Linux. A warm program (an interpreter that has imported its modules, a
service that has loaded its data) should also start from a template instead
of repeating its warm-up.

## What Changes

- Floating-point/SIMD state per cell (AArch64 `V0`–`V31`, `FPCR`, `FPSR`;
  x86_64 SSE through `FXSAVE`), switched lazily (only when another cell takes
  the CPU), part of every snapshot and fork. Snapshot format version 2.
- The system calls a static glibc program needs: `mprotect`, `prlimit64`
  and `getrlimit`, `set_robust_list`, `fstat`/`newfstatat` of the standard
  descriptors, `readlink(at)`, the uid/gid family, `getppid`, `sigaltstack`,
  `madvise`, a single-threaded `futex`; `rseq` declined quietly (libc falls
  back, as on kernels without it).
- Real programs as test cells: C sources in `cells/c/`, linked statically
  with the system glibc by `xtask` (native gcc, or the cross gcc when the
  architecture differs), embedded like the Rust cells and held to the same
  expectations, which the differential check verifies against Linux.
- A warm workload (`libc_warm`): frozen after its warm-up, every request is a
  fork. Metrics `workload.cold_us` and `workload.warm_us`.
- Lazy resume works with `mprotect` (pages brought in first) and unmapping
  (pending pages in an unmapped range are discarded).

### Not in this change: MicroPython

The roadmap names MicroPython as H6's workload. Building it means running
its upstream build (Makefiles) from a fresh clone; in the environment this
change was developed in, that step needs the maintainer's explicit approval.
The same claim (unmodified real programs on a real libc, identical output to
Linux, warm templates) is validated here with glibc programs. Adding
MicroPython later is a build step in `xtask` plus test scripts: no kernel
change is expected beyond system calls it may reveal.

### Out of scope

Files and a file system, threads, signals delivery, dynamic linking, AVX
(XSAVE) state.

## Capabilities

### New Capabilities

- `workloads`: real programs as cells, held to Linux's behavior.

### Modified Capabilities

- `linux-abi`: the calls above.
- `cells`: FP/SIMD state, part of snapshots and forks.
- `benchmarks`: warm and cold workload metrics.
