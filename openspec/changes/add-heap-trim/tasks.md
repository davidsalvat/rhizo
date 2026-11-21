# Tasks

## 1. Heap

- [x] 1.1 In-place merge sort of a free list and `Heap::trim` (D1); host tests `sort_orders_a_free_list`, `a_page_with_a_live_block_stays`, and trims in `allocations_never_overlap` (every page back at the end); verify `cargo test -p rhizo-mm heap` and `cargo +nightly miri test -p rhizo-mm -- heap::`
- [x] 1.2 `Heap::trim_if_due` (D2); host tests `whole_free_pages_go_back_when_due`, `a_small_heap_is_never_trimmed_when_due`; verify `cargo test -p rhizo-mm heap`

## 2. Kernel

- [x] 2.1 `cell::end` calls `heap::trim_if_due` once the cell is dropped; the out-of-memory push reports `heap_peak_bytes`, and `memory_full_of_cells_is_refused_cleanly` checks the heap gave the spike back; verify `cargo xtask test` and `cargo xtask --arch x86_64 test`
- [x] 2.2 The frame-return tests compare free frames plus the heap's (`free_or_in_heap`, D4); verify `cargo xtask test --only ktest-smp` on both architectures, twice on AArch64
- [x] 2.3 `PageTable::free_empty_tables`, called by the window when it unmaps (D3); host test `empty_tables_are_freed_below_the_upper_levels`; `large_heap_allocations_survive_fragmentation` now expects every frame back; verify `cargo xtask test --only ktest` on both architectures

## 3. Measure

- [x] 3.1 Metric `density.oom_heap_kept_bytes` (`xtask`, `bench/README.md`); server workload and `--icount-only` before and after on both architectures (design, Measurements); verify `cargo xtask bench --workload server` and `cargo xtask bench --icount-only`
