# Tasks

## 1. Kernel build reused

- [x] 1.1 `kernel/build.rs` watches an optional cell only when it is built, and `RHIZO_MICROPYTHON_SRC`, `RHIZO_CPYTHON_SRC` (D1); `check_kernel_reused` in the `control` test step and host test `the_kernel_s_freshness_is_read_from_cargo_messages`; verify `cargo xtask check`, and a second `rhizo up` prints no `Compiling rhizo-kernel`

## 2. Cell identifiers

- [x] 2.1 The control kernel's table numbers the cells it starts and forks (D2); the `control` test step checks that `fork N 2` after a suspend and resume gives `N+1` and `N+2`; verify `cargo xtask --arch x86_64 test --only control`

## 3. State directory

- [x] 3.1 `Dir::clean`, called by the server when the VMM ends and by `rhizo up` (D3); host test `cleaning_leaves_only_the_console_log`, and the `control` test step checks the directory after `down`; usage text and `docs/demo.md`; verify `cargo xtask check` and `cargo xtask --arch x86_64 test`
