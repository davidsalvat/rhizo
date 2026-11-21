# Tasks

## 1. Reproduce

- [x] 1.1 Inject a 2 ms stall in `run_slice` between a blocking call and `threads.next(now_ns())` (local only, not committed); verify `cargo xtask test --only ktest` fails with `suspended 37 times` in `suspend_resume_after_every_step_is_invisible_eager`
- [x] 1.2 Time the test's rounds (local only); verify that `Snapshot::capture`, and in it the zeroing of its buffer, takes most of each variant's time

## 2. Fix

- [x] 2.1 `counter` yields after every step (D1); verify the stall of 1.1 no longer fails the test (41 and 40 suspensions), and its output on Linux is unchanged (`cargo xtask check`)
- [x] 2.2 `Snapshot::capture` fills a zeroed buffer of its final size (D2); verify `cargo test -p rhizo-snapshot` and each variant's time under `cargo xtask test --only ktest` on both architectures

## 3. Gates

- [x] 3.1 Verify `cargo xtask --arch x86_64 bench --icount-only --vmm qemu-microvm --no-reference --runs 1` before and after: `cell.suspend_insns` within the gate, no other metric moves
- [ ] 3.2 Verify `cargo xtask check`, `cargo xtask --arch x86_64 test`, `cargo xtask test`, and `cargo xtask test --only ktest` ten times on each architecture
