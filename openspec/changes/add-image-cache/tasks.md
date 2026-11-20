# Tasks

## 1. Library

- [x] 1.1 `PageTable::clone_lower_half` (D2): host test of sharing, private writable copies, untouched source and rollback when frames run out, and `Clone` in the copy-on-write model check; verify `cargo test -p rhizo-mm cow`
- [x] 1.2 `rhizo_cell::load` and `rhizo_cell::start`, `spawn` built from them (D1); host test that a cell started on a copy of a loaded space equals a spawned one and keeps its writes; `rhizo_cell::images::Images` with a host test of the order it drops in (D3); verify `cargo test -p rhizo-cell`

## 2. Kernel

- [x] 2.1 Executables kept loaded in `kernel/src/cell.rs`: `image_space`, `ImageFrames`, dropping before a cell is refused (D3, D4); instruction cache synchronized once per kept executable (D5); `Host::spawn` takes `&'static` images; verify `cargo xtask check`
- [x] 2.2 In-kernel tests `an_image_spawned_again_starts_fresh` (with `same_addr` checking its static starts fresh), `kept_images_give_way_to_cells`, `a_cell_from_a_kept_image_suspends_resumes_and_forks`; free-memory comparisons made with no executable kept; verify `cargo xtask test` and `cargo xtask test --only ktest-smp` on both architectures

## 3. Benchmarks and documentation

- [x] 3.1 `bench/README.md` describes what `cell.spawn_us` and the cold workloads now measure (D6); verify the CI benchmark gate passes, with `cell.spawn_us`, `cell.run_us` and `workload.cpython_cold_us` lower than on `main` and no metric worse
