# Tasks

## 1. The lock

- [x] 1.1 `SpinLock::lock` and its guard, `Global::lock`; verify `cargo test -p rhizo-core sync`
- [x] 1.2 `Sockets` accessor in `rhizo-abi`; `poll`, `select` and `epoll_wait` lock only for sockets (D1); verify `cargo test -p rhizo-abi only_calls_on_sockets_reach_them`
- [x] 1.3 `CallNet` in the kernel (D1); verify `cargo xtask test` and `--only ktest-smp` on both architectures

## 2. Measuring

- [x] 2.1 `cell.syscall_scaling_pct` (D3); verify `cargo test -p xtask smp_line_is_parsed` and a local `cargo xtask bench --vmm qemu-microvm`
- [x] 2.2 The metric with the lock taken eagerly and lazily, in the pull request; then the metric in the `ci` baseline from two agreeing CI runs; verify the CI benchmark job
- [x] 2.3 `bench/README.md`: the metric; verify by reading it next to a CI run's output
