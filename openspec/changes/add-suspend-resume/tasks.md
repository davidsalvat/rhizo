# Tasks

## 1. Format and memory walks

- [x] 1.1 `rhizo-mm`: walk of the user half (address, physical page, size, permissions) with host tests on both formats
- [x] 1.2 Create `rhizo-snapshot`: snapshot type, encode/decode (D2), capture from a page table and restore (eager), host tests: round trip, determinism, truncation, corruption, version and machine errors; add it to the dependency table
- [x] 1.3 `rhizo-cell`: pager hook in user-memory access; `rhizo-abi` uses it; host tests

## 2. Kernel

- [x] 2.1 `Host::suspend` and `Host::resume` (eager and lazy), lazy page-in on cell faults and through the pager; fault address from both architectures
- [x] 2.2 Test cells and in-kernel tests: equivalence after every step (both modes), memory returned on suspend, untouched pages not restored, many suspended cells, wrong-architecture snapshot rejected

## 3. Benchmarks and documentation

- [x] 3.1 `bench-cell`: suspend/resume metrics for an 8 MiB cell; `xtask bench` records them
- [x] 3.2 README roadmap (H4), AGENTS.md code map, `cargo xtask ci` on both architectures
