# Tasks

## 1. Reproduce

- [x] 1.1 Hostile cell `sharer` (maps one file page 70000 times) and in-kernel test `one_page_mapped_without_end_cannot_stop_the_kernel`; verify it panics the kernel ("more than 65535 cells share a frame") on `main` with `cargo xtask --arch x86_64 test --only ktest`

## 2. Fix

- [x] 2.1 `Frames::try_share` (default: `share`), with bounded counts in `CountingFrames` (`max_refs`) and in the kernel's frames (D2); verify `cargo xtask check`
- [x] 2.2 `PageCache::get` copies a page whose kept frame is full (D1); host test `a_page_shared_without_end_gets_copies`; verify `cargo test -p rhizo-abi pages`
- [x] 2.3 The in-kernel test passes, the other cell finishes and every frame comes back; verify `cargo xtask test` and `cargo xtask test --only ktest-smp` on both architectures
