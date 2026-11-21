# Proposal

## Why

Node.js runs in a cell only with `--jitless` (design D6 of
`add-node-workload`): V8 13.6 on Linux has no W^X mode. Without memory
protection keys it makes its whole code range (512 MiB on x86_64, 128 MiB
on AArch64) readable, writable and executable with one `mprotect`, then
writes and runs code there freely, and cells refuse writable and
executable memory. Without its JIT, V8 runs JavaScript in its interpreter
only: the cell answers, but slowly, and every program that brings its own
JIT (V8, LuaJIT, PyPy, the JVM) is in the same place.

## What Changes

- **Write-or-execute pages.** `mmap` of anonymous private memory and
  `mprotect` accept `PROT_WRITE|PROT_EXEC`; the page tables still never
  hold a page both writable and executable. Such a page carries a software
  bit in its entry (x86_64 bit 10, AArch64 bit 56; the copy-on-write mark
  keeps bit 9 and bit 55) and is in one of two states: *writing*
  (writable, or copy-on-write, and not executable) or *running* (read-only
  and executable). An instruction fetch from a writing page makes it
  running: the entry changes, the old translation is flushed from every
  CPU that runs the cell (broadcast `TLBI` on AArch64, the shootdowns of
  `add-parallel-threads` on x86_64), and on AArch64 the data cache is
  cleaned and the instruction cache invalidated for the page; then the
  thread retries. A write to a running page, by the cell or by a system
  call on its behalf, makes it writing again through the copy-on-write
  path (a frame still shared is copied first).
- **Fault kinds.** Both architectures tell an instruction fetch refused by
  permissions from a write refused by permissions: the x86_64 page-fault
  error code (`P` and `I/D`), the AArch64 syndrome (instruction abort from
  EL0 with a permission fault).
- **Write-or-execute memory without frames yet.** Memory asked writable
  and executable with no pages behind it (an anonymous `mmap`, or
  `mprotect` of reserved addresses) is not backed at once: its range is
  kept, and each page gets a zeroed frame, writing, the first time the cell
  or a system call touches it. V8's 512 MiB code range then costs what V8
  writes, not 512 MiB per cell and per snapshot. At most 64 ranges per
  cell; `munmap`, `MAP_FIXED` and any other `mprotect` drop what they
  cover.
- **Cell state.** The bit and the ranges are cell state: the snapshot
  format moves to version 9 (a permission bit for write-or-execute pages,
  the ranges after the threads), forks and templates keep both, and
  copy-on-write treats a write-or-execute page like any page that may be
  written.
- **AArch64 cache maintenance from EL0.** `SCTLR_EL1.UCT` and `UCI` are
  set on every CPU, as Linux sets them: V8 (and GCC's `__clear_cache`)
  reads `CTR_EL0` and cleans and invalidates caches by address after
  writing code; without them those instructions trap and the cell is
  killed.
- **Node.js with its JIT.** The Node.js tests (`node_answers_as_on_linux`,
  `a_node_cell_suspends_resumes_and_forks`) and the differential check run
  Node.js without `--jitless`. The benchmarks keep the `--jitless`
  figures and add `workload.node_jit_cold_us`,
  `workload.node_jit_warm_us` and `workload.node_jit_flips` (state changes
  of write-or-execute pages in one cold run).

### Out of scope

Writable and executable file mappings (still `EACCES`: nothing asks for
them); two mappings of the same frames (V8 does that only on Apple
platforms); memory protection keys; giving `madvise(MADV_DONTNEED)` an
effect.

## Capabilities

### Modified Capabilities

- `virtual-memory`: write-or-execute pages, never writable and executable
  in the page tables.
- `linux-abi`: `mmap` and `mprotect` accept writable and executable
  anonymous memory, backed on first touch.
- `isolation`: write-or-execute ranges are bounded; snapshots from outside
  still never resume writable and executable pages; changes stay the
  cell's; AArch64 cells may maintain caches by address.
- `snapshots`: format version 9 carries the bit and the ranges.
- `fork`: forks and templates keep write-or-execute pages and ranges.
- `workloads`: Node.js runs with its JIT.
- `benchmarks`: Node.js JIT metrics next to the `--jitless` ones.

## Impact

`crates/rhizo-mm` (`PageFlags::WRITE_OR_EXEC`, both entry formats,
`resolve_write`, `resolve_exec`), `crates/rhizo-cell` (`space::WxRanges`,
mapping and protecting write-or-execute memory), `crates/rhizo-abi`
(`mmap`, `mprotect`, `munmap`, `exec`, user-memory access, `Context::wx`),
`crates/rhizo-snapshot` (version 9), `kernel/src/cell.rs` (fault path,
ranges in `Live`, fork, templates, suspend and resume, benchmark),
`kernel/src/arch/*/user.rs` (`exec_denied`, `SCTLR_EL1`),
`kernel/src/tests.rs`, `cells/c/libc_jit.c`, `cells/expected.rs`,
`xtask/src/bench.rs`, `bench/README.md`, `docs/security.md`. Hot paths
(system calls, switches, copy-on-write faults) gain no instruction: the
new checks sit on fault paths that used to end the cell.
