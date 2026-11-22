# Tasks

## 1. Diagnose

- [x] 1.1 `run_to_pause` errors name the output and the ending; `node.rs`, `fork.rs` and `packages.rs` report the error alone (D2); verify `cargo xtask --arch x86_64 test --only ktest` shows `cell … ended before it paused (Exit(0)), having printed …` for the reproduction of 1.2
- [x] 1.2 Reproduce: mode `pause` of `threads` and `a_threaded_pause_is_seen_next_to_busy_cells`; verify it fails without D1 on x86_64; with three `spin` cells beside the template of `copies_draw_different_random` (not committed), verify `RHIZO_NODE=1 cargo xtask --arch x86_64 test --only ktest-node` fails with `printed past its pause`

## 2. Fix

- [x] 2.1 `Scheduler::take`, with host tests of it and of a model of the host stepping a held cell (D1); verify `cargo test -p rhizo-sched`
- [x] 2.2 `run_to_pause` runs the held cell first once its line is out (D1); verify `cargo xtask --arch x86_64 test --only ktest` passes and that `ktest-node` passes with and without the three `spin` cells of 1.2

## 3. Gates

- [x] 3.1 Verify `openspec validate fix-node-pause --strict`, `cargo xtask check`, and `cargo xtask ci` on both architectures
- [x] 3.2 No benchmark regression: verify `cargo xtask bench --icount-only`
