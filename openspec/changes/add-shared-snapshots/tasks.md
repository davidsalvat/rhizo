# Tasks

## 1. Page tables

- [x] 1.1 `PageTable::unshare_tables` (D1), sharing `private_table` with `walk_private`; host test `unshare_tables_leaves_pages_shared` and a `Hold` step in the copy-on-write model check (a held page never changes); verify `cargo test -p rhizo-mm cow`
- [x] 1.2 `PageTable::map` maps a writable page with `COPY_ON_WRITE` copy-on-write (D4); verify `cargo test -p rhizo-mm`

## 2. Snapshots

- [x] 2.1 `Snapshot::capture` refers to shared frames (records marked in their first reserved byte, D2), releases them on drop (D3); `restore_page` maps them again (D4); `export` writes their contents; `heap_bytes`, `shared_bytes`; host tests `shared_pages_are_referred_to_*` and `records_that_refer_to_frames_are_refused`; verify `cargo test -p rhizo-snapshot`
- [x] 2.2 The kernel captures under the lock of shared tables, releases through its frames, counts only the snapshot's own pages for memory accounting; in-kernel test `a_suspended_fork_refers_to_its_template`; verify `cargo xtask test` and `cargo xtask --arch x86_64 test`

## 3. Measure

- [x] 3.1 Laptop workload before and after on both architectures (design, Measurements); exact instruction counts (`--icount-only`) before and after on both; verify `cargo xtask bench --workload laptop` and `cargo xtask bench --icount-only`
