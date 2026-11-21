# Tasks

## 1. Measure

- [x] 1.1 Kernel compilations and their time for `check`, `test` and `ci` on both architectures, before the change, after `touch kernel/src/main.rs` (design *Context*); verify each count against the `Compiling rhizo-kernel` lines of the run

## 2. Kernel

- [x] 2.1 `rhizo_core::options::self_check`, after the existing functions, with host tests (D1); verify `cargo test -p rhizo-core self_check`
- [x] 2.2 `SELF_CHECKS` and `tests::chosen` in `kernel/src/tests.rs`, the self-check helpers under `ktest`, the eight `ktest-*` features removed (D1); verify `cargo xtask check`
- [x] 2.3 Kernels without `ktest` unchanged (D5); verify the loadable bytes of plain, `bench-cell`, `control`, `bench-cell,profile` and `bench-cell,sampling`, release and debug, on both architectures, are identical before and after

## 3. xtask

- [x] 3.1 `RunOptions::selfcheck` and `selfcheck=` on every VMM; one test kernel for `ktest`, `ktest-smp` and the self-checks; 7 lint sets in `ci` (D4); verify `cargo xtask check`
- [x] 3.2 Every self-check passes as before, and fails when its option is wrong; verify `cargo xtask test --only ktest-<name>` for the eight names on both architectures (one kernel compiled per architecture), and a misspelled option reported as `unknown self-check`

## 4. Verify and document

- [x] 4.1 `.agents/skills/kernel-test/SKILL.md`: adding a self-check; verify by reading
- [x] 4.2 `cargo xtask --arch x86_64 test` and `cargo xtask test`, and the counts and times after the change (design *Results*); verify every step passes and 3 kernels compile per architecture
- [x] 4.3 `-icount` gates; verify `cargo xtask --arch x86_64 bench --icount-only --env ci --check` and `cargo xtask bench --icount-only --env ci-arm --check` pass
- [x] 4.4 `openspec validate fewer-kernel-builds --strict`
