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
- **Memory**: a cell may hold 128 MiB of file data instead of 64 MiB (the
  packed tree is 65 MiB: OpenBLAS alone is 24 MiB); the test VMs grow from
  128 MiB to 1 GiB and the cell benchmark VM from 256 to 512 MiB: numpy's
  files are embedded in the kernel image, decoded into the heap and kept in
  the page cache once mapped, and a snapshot of a cell that imported numpy
  copies them. The memory pressure test, which asked for more than a
  128 MiB VM holds, sizes its cells by the free memory instead.

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
- `files`: the bound on a cell's file data.

## Impact

`crates/rhizo-fs` (`DATA_MAX`), `xtask/src/packages.rs` (new), `xtask/src/{cpython,dynamic,differential,bench,compare,lib,vmm}.rs`,
`cells/python/{numpy_demo,requests_demo,pack_stdlib}.py`,
`cells/expected.rs`, `kernel/build.rs`, `kernel/src/{cell,tests}.rs`,
`bench/host/linux_numpy.c`, `bench/host/numpy_forkserver.py`,
`bench/README.md`, `.github/workflows/ci.yml` (the CPython cache key
covers the pinned wheels). No change to the loader or the system calls
was needed: C.1's run numpy and requests as they are.
