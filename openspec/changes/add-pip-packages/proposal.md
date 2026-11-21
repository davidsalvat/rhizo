# Proposal

## Why

Agents need the packages everyone installs with pip: the vision's
templates include "data analysis with numpy", and most agent tools speak
HTTP through `requests`. The CPython cell is a static executable with a
fixed set of C modules: it cannot load a package's C extension, and numpy
is mostly C (and OpenBLAS). Dynamic linking is now on main
(`add-dynamic-linking`): `PT_INTERP`, `MAP_PRIVATE` file mappings and the
page cache let a dynamically linked interpreter open shared objects with
`dlopen`. Roadmap C.2 is to make packages from PyPI work in a cell the way
`pip install` makes them work on Linux, and to measure what a template that
imported numpy once saves.

## What Changes

- **A dynamically linked CPython**: `xtask` builds the pinned CPython
  release a second time, linked dynamically (its standard library's C
  modules as shared objects in `lib-dynload`, its C API exported for
  extensions), and installs it into a staging tree. The static CPython cell
  and its metrics are unchanged.
- **Wheels from PyPI**: pinned manylinux wheels (numpy 2.5.4 with its
  OpenBLAS; requests 2.34.2, urllib3 2.8.0, idna 3.20,
  charset-normalizer 3.5.2, certifi 2026.10.23), each with its SHA-256,
  downloaded by `xtask` once and installed into that CPython's
  `site-packages` as pip installs them (a wheel is a zip archive laid out
  as installed). Native architecture only, as CPython.
- **Packing**: `pack_stdlib.py` also packs the `site-packages` modules the
  scripts import (compiled), the data files of their packages, the
  libraries wheels vendor (`numpy.libs`) and the packages' metadata; the
  dynamic packing finds a library a file's run path names relative to the
  file (`$ORIGIN`) in the tree. The result is `packages-files`, a cell file
  system with CPython, `ld.so` and every library they need.
- **CPU features on AArch64**: OpenBLAS faulted without `HWCAP_CPUID`
  (it falls back to `/proc` and `/sys`, which cells lack, and does not
  check `fopen`). Cells now get `AT_HWCAP` with FP, Advanced SIMD and
  `HWCAP_CPUID`, and the kernel answers their reads of ID registers
  (`MIDR_EL1`...) as Linux does, emulating the instruction EL0 cannot
  run. x86_64 is unchanged (`CPUID` runs in ring 3).
- **Tests**: in-kernel tests that numpy's computation (array operations, a
  linear solve, a ufunc, a matrix product through OpenBLAS) prints what it
  prints on Linux, that `requests.get` reaches the host's fake model
  through the network card, and that a cell that imported numpy suspends,
  resumes, forks and serves as a template; the differential check runs
  both scripts natively; a parser test for the new benchmark line.
- **Metrics**: `workload.numpy_cold_us` (start the packages' CPython,
  import numpy, compute, exit) and `workload.numpy_warm_us` (the same
  forked from a template that imported numpy); the same two for CPython as
  a Linux process in the comparison's Firecracker guest.
- **Kernels of their own**: as for Node.js (`add-node-workload`), only
  kernels built with the `packages` feature embed the 65 MiB tree; with
  `ktest` they run the package tests only (step `ktest-packages`, 1 GiB, 2
  vCPUs), with `bench-cell` they measure numpy only (1 GiB). Every other
  kernel and VM is unchanged.

### Out of scope

pip itself (`ensurepip`, downloads from a cell), HTTPS (TLS needs the
cells' entropy and certificates handled end to end; certifi's bundle is
packed but unused), packages other than these, and building wheels from
source.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workloads`: packages from PyPI in a cell.
- `benchmarks`: numpy metrics.
- `comparisons`: numpy as a Linux process in the same VM.
- `kernel-testing`: the package tests in their own VM.
- `linux-abi`: CPU features as Linux tells them (AArch64 `AT_HWCAP`, ID
  registers read at EL0).

## Impact

`crates/rhizo-cell` (`AT_HWCAP`),
`crates/rhizo-core` (`IdRegisterRead`), `kernel/src/arch/*/user.rs`
(`UserFault::emulate`), `xtask/src/packages.rs` (new),
`xtask/src/{cpython,dynamic,differential,bench,compare,lib}.rs`,
`cells/python/{numpy_demo,requests_demo,pack_stdlib}.py`,
`cells/c/libc_hwcap.c`, `cells/expected.rs`, `kernel/{build.rs,Cargo.toml}`,
`kernel/src/{cell,tests}.rs`,
`bench/host/linux_numpy.c`, `bench/host/numpy_forkserver.py`,
`bench/README.md`, `.github/workflows/ci.yml` (the CPython cache key
covers the pinned wheels). No change to the loader or the system calls
was needed: C.1's run numpy and requests as they are.
