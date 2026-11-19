# Tasks

## 1. Memory

- [x] 1.1 `rhizo-mm`: reference-count trait and dense implementation; refcount-aware destroy; host tests
- [x] 1.2 `rhizo-mm`: table-level write permission and a copy-on-write mark in both entry formats; effective permissions in `translate`
- [x] 1.3 `rhizo-mm`: lazy fork (D2), write-fault resolution, path unsharing before map/unmap; property tests against a model on both formats
- [x] 1.4 `rhizo-cell`/`rhizo-abi`: space operations unshare first; the pager resolves copy-on-write writes from system calls

## 2. Kernel

- [x] 2.1 Write-permission faults decoded on both architectures; `Host::fork`, copy-on-write resolution, frames freed through the counts
- [x] 2.2 Templates: `Host::freeze`, `Host::spawn_from_template`
- [x] 2.3 Test cells and in-kernel tests: own values, system call into a shared page, child equivalence, parent ending first, forks of forks, all frames returned

## 3. Benchmarks and documentation

- [x] 3.1 `bench-cell`: fork of a 50 MiB cell, fork plus touch, page-table frames of a fork, spawn from template; `xtask bench`
- [x] 3.2 README roadmap (H5), AGENTS.md, `cargo xtask ci` on both architectures
