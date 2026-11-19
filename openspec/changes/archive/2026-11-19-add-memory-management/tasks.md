# Tasks

## 1. Hardware interfaces

- [x] 1.1 Add `PhysicalMemory` and `Tlb` traits to `rhizo-hal` with host doubles (`FakePhysicalMemory`, `RecordingTlb`); verify host tests read back written values and recorded flushes

## 2. Core memory logic (host-tested)

- [x] 2.1 Parse the PVH memory map in `rhizo-core` (`memory::memmap`); verify host tests cover RAM and reserved entries, a missing map (version 0 or zero entries) and the entry cap
- [x] 2.2 Implement the bitmap frame allocator (`memory::frames`): free regions, reservations, single and contiguous allocation, free with misuse detection; verify unit tests plus a proptest that random alloc/free sequences never return duplicate, misaligned or reserved frames
- [x] 2.3 Implement page tables (`memory::paging`): map 4 KiB and 2 MiB pages with flags, translate, unmap with TLB flush, already-mapped and non-canonical errors; verify host tests against `FakePhysicalMemory`
- [x] 2.4 Implement the size-class heap (`memory::heap`) on a `PageSource` trait; verify unit tests and a proptest checking alignment, non-overlap (with written patterns) and reuse after free
- [x] 2.5 Implement exception report formatting (`exception`): names for vectors 0–31, page-fault error decoding; verify host tests check exact report lines

## 3. Kernel: CPU setup and higher half

- [x] 3.1 Link the kernel in the higher half with physical load addresses, a low `.boot` section, section boundary symbols and a guarded kernel stack; verify `readelf -lW` shows physical addresses at 1 MiB and virtual addresses at the kernel base
- [x] 3.2 Update the trampoline: stage-1 tables (identity, direct map, kernel base), `EFER.NXE`, `CR0.WP`, jump to the higher half, pass start info and entry TSC to `kernel_main`; verify `cargo xtask run` boots and prints the banner
- [x] 3.3 Install GDT, TSS (IST1 double-fault stack) and IDT with exception stubs; verify the `ktest-invalid-opcode` self-check prints the invalid opcode report and fails the VM

## 4. Kernel: memory setup

- [x] 4.1 Wire memory map, frame allocator and stage-2 page tables (direct map, W^X kernel sections, guard page, no identity map) at boot and print the memory line; verify boot output and in-kernel tests for lower-half emptiness, W^X flags and direct-map access
- [x] 4.2 Install the global allocator backed by frames; verify in-kernel tests for Box, large Vec, alignment and reuse
- [x] 4.3 Add in-kernel tests for frame allocation, contiguous runs and map/use/unmap of a page; verify `cargo xtask test` passes
- [x] 4.4 Add `ktest-page-fault`, `ktest-stack-overflow` and `ktest-oom` self-checks to the kernel and `xtask`, and require the `memory:` line in the boot check; verify each self-check fails the VM with the expected report

## 5. Integration and documentation

- [ ] 5.1 Update README (roadmap, layout) and AGENTS.md (memory layout notes); verify `cargo xtask ci` passes locally and in CI on the pull request

## Workflow follow-up

- Archive this change with `openspec archive add-memory-management` after it is merged.
- Mark milestone H1 as done in the README roadmap.
